#![allow(unsafe_op_in_unsafe_fn)]

use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, madvise};

use crate::{
    AVERAGE_BLOCK_TIMES_PTHREAD, HEADER_SIZE, OX_CURRENT_STAMP, OxHeader,
    slab::{
        NUM_SIZE_CLASSES, SIZE_CLASSES, get_size_4096_class,
        global::GlobalHandler,
        thread_local::{THREAD_REGISTER, ThreadLocalEngine},
    },
    trim::{
        TimeDecay,
        thread::{LAST_PRESSURE_CHECK, PTRIM_DECAY},
    },
    va::bootstrap::SHUTDOWN,
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
            let usage = (*engine).tls[class].usage.load(Ordering::Relaxed);
            (usage, true)
        }
    }

    unsafe fn get_numa_id(&self, engine: *mut ThreadLocalEngine) -> (usize, bool) {
        if engine.is_null() {
            (0, false)
        } else {
            let numa_id = (*engine).numa_node_id;
            (numa_id, true)
        }
    }

    pub unsafe fn trim(&self, pad: usize) -> (i32, usize) {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return (0, 0);
        }
        let pressure = LAST_PRESSURE_CHECK.load(Ordering::Relaxed);
        let force_trim = pressure > 85;

        let mut node = THREAD_REGISTER.load(Ordering::Acquire);
        if node.is_null() {
            return (0, 0);
        }

        let mut avg = 0;
        let mut total = 0;
        let mut total_freed = 0;

        let timing = AVERAGE_BLOCK_TIMES_PTHREAD.load(Ordering::Relaxed);
        let half_timing = if timing / 2 == 0 { 1 } else { timing / 2 };
        let class_4096 = get_size_4096_class();

        while !node.is_null() {
            let engine = (*node).engine.load(Ordering::Acquire);

            if !engine.is_null() {
                let (numa_id, is_ok) = self.get_numa_id(engine);
                if !is_ok {
                    continue;
                }

                for class in 0..class_4096 {
                    if total_freed >= pad && pad != 0 {
                        return (1, total_freed);
                    }

                    let (usage, is_ok) = self.get_usage(engine, class);
                    if !is_ok {
                        break;
                    }

                    for _ in 0..usage / 2 {
                        let (cache, is_ok) = self.pop_from_thread(engine, class);
                        if !is_ok || cache.is_null() {
                            break;
                        }

                        let life_time = OX_CURRENT_STAMP
                            .load(Ordering::Relaxed)
                            .saturating_sub((*cache).life_time);

                        if life_time > half_timing {
                            let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);
                            (*cache).life_time = current;
                            GlobalHandler.push_to_global(class, numa_id, cache, cache, 1);
                            continue;
                        }

                        let push = self.push_to_thread(engine, class, cache);
                        if !push {
                            GlobalHandler.push_to_global(class, numa_id, cache, cache, 1);
                            break;
                        }
                    }
                }

                for class in class_4096..NUM_SIZE_CLASSES {
                    if total_freed >= pad && pad != 0 {
                        return (1, total_freed);
                    }

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

                        if life_time != 0 {
                            avg += life_time;
                            total += 1;
                        }

                        if life_time > timing {
                            (*cache).next = to_trim;
                            to_trim = cache;
                            continue;
                        } else if life_time > half_timing {
                            let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);
                            (*cache).life_time = current;
                            GlobalHandler.push_to_global(class, numa_id, cache, cache, 1);
                            continue;
                        }

                        let push = self.push_to_thread(engine, class, cache);
                        if !push {
                            GlobalHandler.push_to_global(class, numa_id, cache, cache, 1);
                            break;
                        }
                    }

                    while !to_trim.is_null() {
                        let next = (*to_trim).next;
                        let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);
                        if (total_freed <= pad || pad == 0) || force_trim {
                            self.release_memory(to_trim, SIZE_CLASSES[class]);
                            (*to_trim).used_before = 0;
                            total_freed += SIZE_CLASSES[class];
                        }
                        (*to_trim).life_time = current;
                        GlobalHandler.push_to_global(class, numa_id, to_trim, to_trim, 1);
                        to_trim = next;
                    }
                }
            }

            node = (*node).next.load(Ordering::Acquire);
        }

        if total > 0 {
            let avg = (avg / total).max(100).min(10000);
            AVERAGE_BLOCK_TIMES_PTHREAD.store(avg, Ordering::Relaxed);
            PTRIM_DECAY.swap(TimeDecay::decide_on(avg) as u8, Ordering::AcqRel);
        }

        if total_freed >= pad {
            (1, total_freed)
        } else {
            (0, total_freed)
        }
    }

    #[inline]
    fn release_memory(&self, header_ptr: *mut OxHeader, size: usize) {
        unsafe {
            const PAGE_SIZE: usize = 4096;
            const PAGE_MASK: usize = !(PAGE_SIZE - 1);

            let header = header_ptr as usize;
            let user_start = header + HEADER_SIZE;
            let user_end = user_start + size;

            let page_start = (user_start + PAGE_SIZE - 1) & PAGE_MASK;
            let page_end = user_end & PAGE_MASK;

            if page_start >= page_end {
                return;
            }
            let length = page_end - page_start;

            let _ = madvise(page_start as *mut c_void, length, Advice::LinuxDontNeed);
        }
    }
}
