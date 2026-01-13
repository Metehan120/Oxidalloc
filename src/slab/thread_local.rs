#![allow(unsafe_op_in_unsafe_fn)]

use libc::{pthread_getspecific, pthread_key_t, pthread_setspecific};
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};
use std::{
    hint::spin_loop,
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
};

use crate::{
    OxHeader, OxidallocError,
    slab::{
        NUM_SIZE_CLASSES, SIZE_CLASSES,
        global::{GLOBAL, GlobalHandler},
        quarantine::quarantine,
    },
    va::va_helper::is_ours,
};

pub struct ThreadNode {
    pub engine: AtomicPtr<ThreadLocalEngine>,
    pub next: AtomicPtr<ThreadNode>,
}

pub static THREAD_REGISTER: AtomicPtr<ThreadNode> = AtomicPtr::new(null_mut());
pub static THREAD_REGISTER_LOCK: AtomicBool = AtomicBool::new(false);

#[inline(always)]
pub fn lock_thread_register() {
    while THREAD_REGISTER_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }
}

#[inline(always)]
pub fn unlock_thread_register() {
    THREAD_REGISTER_LOCK.store(false, Ordering::Release);
}

// Register a new thread node with the given, need for trimming
unsafe fn register_node(ptr: *mut ThreadLocalEngine) -> *mut ThreadNode {
    let node = match mmap_anonymous(
        null_mut(),
        size_of::<ThreadNode>(),
        ProtFlags::READ | ProtFlags::WRITE,
        MapFlags::PRIVATE,
    ) {
        Ok(mem) => mem,
        Err(err) => OxidallocError::PThreadCacheFailed.log_and_abort(
            null_mut() as *mut c_void,
            "PThread cache creation failed, cannot create thread node: errno({})",
            Some(err),
        ),
    } as *mut ThreadNode;

    std::ptr::write(
        node,
        ThreadNode {
            engine: AtomicPtr::new(ptr),
            next: AtomicPtr::new(null_mut()),
        },
    );

    lock_thread_register();
    let head = THREAD_REGISTER.load(Ordering::Acquire);
    (*node).next.store(head, Ordering::Relaxed);
    THREAD_REGISTER.store(node, Ordering::Release);
    unlock_thread_register();

    node
}

#[inline(always)]
unsafe fn destroy_node(node: *mut ThreadNode) {
    lock_thread_register();
    (*node).engine.swap(null_mut(), Ordering::AcqRel);
    unlock_thread_register();
}

static THREAD_KEY: OnceLock<pthread_key_t> = OnceLock::new();

pub struct ThreadLocalEngine {
    pub cache: [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub usages: [AtomicUsize; NUM_SIZE_CLASSES],
    pub latest_usages: [AtomicUsize; NUM_SIZE_CLASSES],
    pub latest_popped_next: [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub locks: [AtomicBool; NUM_SIZE_CLASSES],
    pub node: *mut ThreadNode,
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub fn prefetch(ptr: *const u8) {
    unsafe { core::arch::x86_64::_mm_prefetch(ptr as *const i8, core::arch::x86_64::_MM_HINT_T0) }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub fn prefetch(ptr: *const u8) {
    unsafe { core::arch::aarch64::_prefetch(ptr, core::arch::aarch64::PLDL1KEEP) }
}

impl ThreadLocalEngine {
    #[inline(always)]
    pub unsafe fn get_or_init() -> &'static ThreadLocalEngine {
        let key = THREAD_KEY.get_or_init(|| {
            let mut key = 0;
            libc::pthread_key_create(&mut key, Some(cleanup_thread_cache));
            key
        });

        let cache_ptr = pthread_getspecific(*key) as *mut ThreadLocalEngine;

        if cache_ptr.is_null() {
            let cache = mmap_anonymous(
                null_mut(),
                size_of::<ThreadLocalEngine>(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            );

            match cache {
                Ok(cache) => {
                    let cache = cache as *mut ThreadLocalEngine;
                    std::ptr::write(
                        cache,
                        ThreadLocalEngine {
                            cache: [const { AtomicPtr::new(std::ptr::null_mut()) };
                                NUM_SIZE_CLASSES],
                            usages: [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES],
                            latest_usages: [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES],
                            latest_popped_next: [const { AtomicPtr::new(std::ptr::null_mut()) };
                                NUM_SIZE_CLASSES],
                            locks: [const { AtomicBool::new(false) }; NUM_SIZE_CLASSES],
                            node: null_mut(),
                        },
                    );
                    unsafe {
                        (*cache).node = register_node(cache);
                    };

                    pthread_setspecific(*key, cache as *mut c_void);
                    return &*cache;
                }

                Err(err) => OxidallocError::PThreadCacheFailed.log_and_abort(
                    0 as *mut c_void,
                    "PThread cache creation failed: errno({})",
                    Some(err),
                ),
            }
        }

        return &*cache_ptr;
    }

    #[inline(always)]
    pub fn lock(&self, class: usize) {
        while self.locks[class]
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
    }

    #[inline(always)]
    pub fn unlock(&self, class: usize) {
        self.locks[class].store(false, Ordering::Release);
    }

    #[inline(always)]
    pub unsafe fn pop_from_thread(&self, mut class: usize, is_trim: bool) -> *mut OxHeader {
        self.lock(class);
        let mut header = self.cache[class].load(Ordering::Relaxed);

        if header.is_null() {
            if SIZE_CLASSES[class] < 1024 * 2 && !is_trim {
                for i in 1..=2 {
                    let needed = class + i;

                    let h = self.cache[needed].load(Ordering::Relaxed);
                    if !h.is_null() {
                        self.unlock(class);
                        self.lock(needed);
                        class = needed;
                        header = self.cache[class].load(Ordering::Relaxed);
                        break;
                    }
                }
            }

            if header.is_null() {
                self.unlock(class);
                return null_mut();
            }
        }

        // Check if the header is ours
        if !is_ours(header as usize) {
            // Try to recover, if fails return null
            if !quarantine(Some(self), header as usize, class, true) {
                self.cache[class].store(null_mut(), Ordering::Relaxed);
                self.usages[class].store(0, Ordering::Relaxed);
                self.unlock(class);

                // Return null, freelist is nulled
                return null_mut();
            };
            let cur_header = self.cache[class].load(Ordering::Relaxed);
            header = cur_header;
        }

        // Check if data is still valid
        if header.is_null() || !is_ours(header as usize) {
            self.unlock(class);
            return null_mut();
        }

        let next = (*header).next;
        if !next.is_null() {
            prefetch(next as *const u8);
        }

        self.cache[class].store(next, Ordering::Relaxed);
        self.latest_popped_next[class].store(next, Ordering::Relaxed);
        let usage = self.usages[class].fetch_sub(1, Ordering::Relaxed);
        self.latest_usages[class].store(usage, Ordering::Relaxed);
        self.unlock(class);

        header
    }

    #[inline(always)]
    pub unsafe fn push_to_thread(&self, class: usize, head: *mut OxHeader) {
        self.lock(class);

        let current_header = self.cache[class].load(Ordering::Relaxed);
        (*head).next = current_header;

        self.cache[class].store(head, Ordering::Relaxed);
        self.usages[class].fetch_add(1, Ordering::Relaxed);

        self.unlock(class);
    }

    #[inline(always)]
    pub unsafe fn push_to_thread_tailed(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        self.lock(class);

        let current_header = self.cache[class].load(Ordering::Relaxed);
        (*tail).next = current_header;

        self.cache[class].store(head, Ordering::Relaxed);
        self.usages[class].fetch_add(batch_size, Ordering::Relaxed);

        self.unlock(class);
    }
}

unsafe extern "C" fn cleanup_thread_cache(cache_ptr: *mut c_void) {
    let cache = cache_ptr as *mut ThreadLocalEngine;
    if cache.is_null() {
        return;
    }
    destroy_node((*cache).node);

    // Move all blocks to global
    for class in 0..GLOBAL.len() {
        (*cache).lock(class);
        let head = (*cache).cache[class].swap(null_mut(), Ordering::AcqRel);
        if !is_ours(head as usize) {
            continue;
        }

        if !head.is_null() {
            let mut tail = head;
            let mut count = 1;
            loop {
                let next = (*tail).next;
                (*tail).life_time = 0;
                if next.is_null() {
                    break;
                }
                if !is_ours(next as usize) {
                    (*tail).next = null_mut();
                    break;
                }
                tail = next;
                count += 1;
            }

            GlobalHandler.push_to_global(class, head, tail, count);
        }
        (*cache).unlock(class);
    }

    let _ = munmap(cache_ptr, size_of::<ThreadLocalEngine>());
}
