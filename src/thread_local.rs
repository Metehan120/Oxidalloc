use std::{
    hint::spin_loop,
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, AtomicUsize, Ordering},
    },
};

use libc::{pthread_getspecific, pthread_key_t, pthread_setspecific};

use crate::{
    MAP, OxHeader, PROT,
    free::is_ours,
    global::{GLOBAL, GLOBAL_USAGE, GlobalHandler},
};

static THREAD_KEY: OnceLock<pthread_key_t> = OnceLock::new();

pub struct ThreadLocalEngine {
    pub cache: [AtomicPtr<OxHeader>; 20],
    pub usages: [AtomicUsize; 20],
}

impl ThreadLocalEngine {
    pub fn get_or_init() -> &'static ThreadLocalEngine {
        unsafe {
            let key = THREAD_KEY.get_or_init(|| {
                let mut key = 0;
                libc::pthread_key_create(&mut key, Some(cleanup_thread_cache));
                key
            });

            let cache_ptr = pthread_getspecific(*key) as *mut ThreadLocalEngine;

            if cache_ptr.is_null() {
                let cache = libc::mmap(null_mut(), size_of::<ThreadLocalEngine>(), PROT, MAP, -1, 0)
                    as *mut ThreadLocalEngine;

                std::ptr::write(
                    cache,
                    ThreadLocalEngine {
                        cache: [const { AtomicPtr::new(std::ptr::null_mut()) }; 20],
                        usages: [const { AtomicUsize::new(0) }; 20],
                    },
                );

                pthread_setspecific(*key, cache as *mut c_void);
                return &*cache;
            }

            return &*cache_ptr;
        }
    }

    pub fn pop_from_thread(&self, class: usize) -> *mut OxHeader {
        unsafe {
            loop {
                let header = self.cache[class].load(Ordering::Acquire);

                if header.is_null() {
                    return null_mut();
                }

                let next = (*header).next;

                if !is_ours(header as *mut c_void) {
                    return null_mut();
                }

                if self.cache[class]
                    .compare_exchange(header, next, Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    return header;
                }

                spin_loop();
            }
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn push_to_thread(&self, class: usize, head: *mut OxHeader) {
        loop {
            let current_head = self.cache[class].load(Ordering::Acquire);
            (*head).next = current_head;

            if self.cache[class]
                .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return;
            }

            spin_loop();
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
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
                return;
            }

            spin_loop();
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cleanup_thread_cache(cache_ptr: *mut c_void) {
    let cache = cache_ptr as *mut ThreadLocalEngine;

    // Move all blocks to global
    for class in 0..GLOBAL.len() {
        let usage = (*cache).usages[class].load(Ordering::Relaxed);
        let head = (*cache).cache[class].swap(null_mut(), Ordering::AcqRel);

        if !head.is_null() {
            let mut tail = head;
            while !(*tail).next.is_null() {
                tail = (*tail).next;
            }

            GlobalHandler.push_to_global(class, head, tail);
            GLOBAL_USAGE[class].fetch_add(usage, Ordering::Relaxed);
        }
    }

    // Free the cache itself
    libc::munmap(cache_ptr, std::mem::size_of::<ThreadLocalEngine>());
}
