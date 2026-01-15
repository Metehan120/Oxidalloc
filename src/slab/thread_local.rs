#![allow(unsafe_op_in_unsafe_fn)]

use libc::{pthread_getspecific, pthread_key_t, pthread_setspecific};
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};
use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
};

use crate::{
    OX_ENABLE_EXPERIMENTAL_HEALING, OxHeader, OxidallocError,
    slab::{
        NUM_SIZE_CLASSES,
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
            0 as *mut c_void,
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

    loop {
        let head = THREAD_REGISTER.load(Ordering::Acquire);
        (*node).next.store(head, Ordering::Relaxed);

        if THREAD_REGISTER
            .compare_exchange(head, node, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            break;
        }
    }

    node
}

unsafe fn destroy_node(node: *mut ThreadNode) {
    (*node).engine.swap(null_mut(), Ordering::AcqRel);
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

static THREAD_KEY: OnceLock<pthread_key_t> = OnceLock::new();

#[repr(C, align(16))]
pub struct TlsBin {
    pub head: AtomicPtr<OxHeader>,
    pub usage: AtomicUsize,
    pub latest_usage: AtomicUsize,
    pub latest_next: AtomicPtr<OxHeader>,
}

pub struct ThreadLocalEngine {
    pub tls: [TlsBin; NUM_SIZE_CLASSES],
    pub node: *mut ThreadNode,
}

impl ThreadLocalEngine {
    #[inline(always)]
    pub unsafe fn get_or_init() -> &'static ThreadLocalEngine {
        let key = THREAD_KEY.get_or_init(|| {
            let mut key = 0;
            libc::pthread_key_create(&mut key, Some(cleanup_thread_cache));
            key
        });
        let tls = pthread_getspecific(*key) as *mut ThreadLocalEngine;

        if tls.is_null() {
            let tls_size = size_of::<TlsBin>() * NUM_SIZE_CLASSES;
            let engine_size = size_of::<ThreadLocalEngine>();
            let total_size = tls_size + engine_size;

            let cache = mmap_anonymous(
                null_mut(),
                total_size,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
            .unwrap_or_else(|err| {
                OxidallocError::PThreadCacheFailed.log_and_abort(
                    null_mut(),
                    "PThread cache creation failed: errno({})",
                    Some(err),
                )
            }) as *mut ThreadLocalEngine;

            std::ptr::write(
                cache,
                ThreadLocalEngine {
                    tls: [const {
                        TlsBin {
                            head: AtomicPtr::new(null_mut()),
                            usage: AtomicUsize::new(0),
                            latest_usage: AtomicUsize::new(0),
                            latest_next: AtomicPtr::new(null_mut()),
                        }
                    }; NUM_SIZE_CLASSES],
                    node: null_mut(),
                },
            );

            (*cache).node = register_node(cache);
            pthread_setspecific(*key, cache as *mut c_void);

            return &*cache;
        }

        &*tls
    }

    #[inline(always)]
    pub unsafe fn pop_from_thread(&self, class: usize) -> *mut OxHeader {
        loop {
            let mut header = self.tls[class].head.load(Ordering::Relaxed);

            if header.is_null() {
                return null_mut();
            }

            // Check if the header is ours
            if !is_ours(header as usize) {
                // Try to recover, if fails return null
                if !quarantine(
                    Some(self),
                    header as usize,
                    class,
                    OX_ENABLE_EXPERIMENTAL_HEALING.load(Ordering::Relaxed),
                ) {
                    self.tls[class].usage.store(0, Ordering::Relaxed);
                    return null_mut();
                }
                header = self.tls[class].head.load(Ordering::Relaxed);
            }

            // Check if data is still valid
            if header.is_null() || !is_ours(header as usize) {
                return null_mut();
            }

            let next = (*header).next;
            if !next.is_null() {
                prefetch(next as *const u8);
            }

            if self.tls[class]
                .head
                .compare_exchange(header, next, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.tls[class].latest_next.store(next, Ordering::Relaxed);
                let usage = self.tls[class].usage.fetch_sub(1, Ordering::Relaxed);
                self.tls[class].latest_usage.store(usage, Ordering::Relaxed);
                return header;
            }
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread(&self, class: usize, head: *mut OxHeader) {
        loop {
            let current = self.tls[class].head.load(Ordering::Relaxed);
            (*head).next = current;

            if self.tls[class]
                .head
                .compare_exchange(current, head, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                self.tls[class].usage.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread_tailed(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        loop {
            let current_header = self.tls[class].head.load(Ordering::Relaxed);
            (*tail).next = current_header;

            if self.tls[class]
                .head
                .compare_exchange(current_header, head, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                self.tls[class]
                    .usage
                    .fetch_add(batch_size, Ordering::Relaxed);
                return;
            }
        }
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
        let head = (*cache).tls[class].head.swap(null_mut(), Ordering::AcqRel);
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
    }

    let tls_size = size_of::<TlsBin>() * NUM_SIZE_CLASSES;
    let engine_size = size_of::<ThreadLocalEngine>();
    let total_size = tls_size + engine_size;

    let _ = munmap(cache_ptr, total_size);
}
