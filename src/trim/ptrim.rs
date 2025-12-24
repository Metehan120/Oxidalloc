#![allow(unsafe_op_in_unsafe_fn)]

use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, madvise};

use crate::{
    HEADER_SIZE, OX_CURRENT_STAMP, OxHeader,
    slab::{
        ITERATIONS, NUM_SIZE_CLASSES, SIZE_CLASSES,
        global::GlobalHandler,
        match_size_class,
        thread_local::{THREAD_REGISTER, ThreadLocalEngine},
    },
    va::{align_to, bitmap::VA_MAP, bootstrap::SHUTDOWN},
};

pub struct PTrim;

impl PTrim {
    unsafe fn pop_from_thread(
        &self,
        cache: *mut ThreadLocalEngine,
        class: usize,
    ) -> (*mut OxHeader, bool) {
        if cache.is_null() {
            return (null_mut(), false);
        }

        let cache_mem = (*cache).pop_from_thread(class);

        if cache_mem.is_null() {
            return (null_mut(), true);
        }

        (*cache_mem).next = null_mut();

        if (*cache_mem).in_use == 1 {
            return (null_mut(), true);
        }

        (cache_mem, true)
    }

    unsafe fn push_to_thread(
        &self,
        engine: *mut ThreadLocalEngine,
        class: usize,
        head: *mut OxHeader,
    ) -> bool {
        if engine.is_null() {
            return false;
        }

        (*engine).push_to_thread(class, head);

        true
    }

    unsafe fn get_usage(&self, engine: *mut ThreadLocalEngine, class: usize) -> (usize, bool) {
        if engine.is_null() {
            (0, false)
        } else {
            let usage = (*engine).usages[class].load(Ordering::Relaxed);
            (usage, true)
        }
    }

    pub unsafe fn trim(&self) {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return;
        }

        let mut node = THREAD_REGISTER.load(Ordering::Acquire);
        if node.is_null() {
            return;
        }

        while !node.is_null() {
            let engine = (*node).engine.load(Ordering::Acquire);

            if !engine.is_null() {
                for class in 9..NUM_SIZE_CLASSES {
                    let mut to_trim: *mut OxHeader = null_mut();

                    let (usage, is_ok) = self.get_usage(engine, class);
                    if !is_ok {
                        break;
                    }

                    for _ in 0..usage {
                        let (cache, is_ok) = self.pop_from_thread(engine, class);
                        if !is_ok || cache.is_null() {
                            break;
                        }

                        let life_time = OX_CURRENT_STAMP
                            .load(Ordering::Relaxed)
                            .saturating_sub((*cache).life_time);

                        if life_time > 10000 {
                            (*cache).next = to_trim;
                            to_trim = cache;
                            continue;
                        } else if life_time > 5000 {
                            let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);
                            (*cache).life_time = current;
                            GlobalHandler.push_to_global(class, cache, cache, 1);
                            continue;
                        }

                        let push = self.push_to_thread(engine, class, cache);
                        if !push {
                            GlobalHandler.push_to_global(class, cache, cache, 1);
                            break;
                        }
                    }

                    while !to_trim.is_null() {
                        let next = (*to_trim).next;

                        if !self.release_memory(to_trim, SIZE_CLASSES[class]) {
                            if !self.push_to_thread(engine, class, to_trim) {
                                GlobalHandler.push_to_global(class, to_trim, to_trim, 1);
                            }
                        }

                        to_trim = next;
                    }
                }
            }

            node = (*node).next.load(Ordering::Acquire);
        }
    }

    #[inline]
    fn release_memory(&self, header_ptr: *mut OxHeader, size: usize) -> bool {
        unsafe {
            const PAGE_SIZE: usize = 4096;
            const PAGE_MASK: usize = !(PAGE_SIZE - 1);

            let header = header_ptr as usize;
            let user_start = header + HEADER_SIZE;
            let user_end = user_start + size;

            let page_start = (user_start + PAGE_SIZE - 1) & PAGE_MASK;
            let page_end = user_end & PAGE_MASK;

            if page_start >= page_end {
                return false;
            }
            let length = page_end - page_start;
            let class = match_size_class(size).unwrap();

            if ITERATIONS[class] == 1 {
                let payload_size = SIZE_CLASSES[class];
                let block_size = align_to(payload_size + HEADER_SIZE, 16);
                let total = block_size * ITERATIONS[class];

                let is_ok =
                    madvise(header_ptr as *mut c_void, total, Advice::LinuxDontNeed).is_ok();

                if is_ok {
                    VA_MAP.free(header_ptr as usize, total);
                }

                return is_ok;
            }

            madvise(page_start as *mut c_void, length, Advice::LinuxDontNeed).is_ok()
        }
    }
}
