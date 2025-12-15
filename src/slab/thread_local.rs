#![allow(unsafe_op_in_unsafe_fn)]

use libc::{pthread_getspecific, pthread_key_t, pthread_setspecific};
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};
use std::{
    hint::spin_loop,
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
};

use crate::{
    OxHeader, OxidallocError,
    slab::{
        ITERATIONS, NUM_SIZE_CLASSES,
        global::{GLOBAL, GLOBAL_USAGE, GlobalHandler},
        quaratine::quarantine,
    },
    va::va_helper::is_ours,
};

static THREAD_KEY: OnceLock<pthread_key_t> = OnceLock::new();

pub struct ThreadLocalEngine {
    pub cache: [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub usages: [AtomicUsize; NUM_SIZE_CLASSES],
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
    pub fn get_or_init() -> &'static ThreadLocalEngine {
        unsafe {
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
                            },
                        );

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
    }

    #[inline(always)]
    pub fn pop_from_thread(&self, class: usize) -> *mut OxHeader {
        unsafe {
            loop {
                let header = self.cache[class].load(Ordering::Acquire);

                if header.is_null() {
                    return null_mut();
                }

                if !is_ours(header as usize) {
                    quarantine(header as usize);
                    if GLOBAL[class]
                        .compare_exchange(header, null_mut(), Ordering::Release, Ordering::Acquire)
                        .is_ok()
                    {
                        GLOBAL_USAGE[class].store(0, Ordering::Relaxed);
                    }
                    return null_mut();
                }

                let next = (*header).next;
                if !next.is_null() {
                    prefetch(next as *const u8);
                }

                if self.cache[class]
                    .compare_exchange(header, next, Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    self.usages[class].fetch_sub(1, Ordering::Relaxed);
                    return header;
                }

                spin_loop();
            }
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread(&self, class: usize, head: *mut OxHeader) {
        loop {
            let current_head = self.cache[class].load(Ordering::Acquire);
            (*head).next = current_head;

            if self.cache[class]
                .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                self.usages[class].fetch_add(1, Ordering::Relaxed);
                return;
            }

            spin_loop();
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread_tailed(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
    ) {
        loop {
            let current_head = self.cache[class].load(Ordering::Acquire);
            (*tail).next = current_head;

            if self.cache[class]
                .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                self.usages[class].fetch_add(ITERATIONS[class], Ordering::Relaxed);
                return;
            }

            spin_loop();
        }
    }
}

unsafe extern "C" fn cleanup_thread_cache(cache_ptr: *mut c_void) {
    if let Some(key) = THREAD_KEY.get() {
        libc::pthread_setspecific(*key, core::ptr::null_mut());
    }

    let cache = cache_ptr as *mut ThreadLocalEngine;
    if cache.is_null() {
        return;
    }

    // Move all blocks to global
    for class in 0..GLOBAL.len() {
        let usage = (*cache).usages[class].load(Ordering::Relaxed);
        let head = (*cache).cache[class].swap(null_mut(), Ordering::AcqRel);

        if !head.is_null() {
            let mut tail = head;
            while !(*tail).next.is_null() {
                (*tail).life_time = 0;
                tail = (*tail).next;
            }

            GlobalHandler.push_to_global(class, head, tail, usage);
            GLOBAL_USAGE[class].fetch_add(usage, Ordering::Relaxed);
        }
    }

    let _ = munmap(cache_ptr, size_of::<ThreadLocalEngine>());
}
