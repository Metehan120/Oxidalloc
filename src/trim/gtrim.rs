#![allow(unsafe_op_in_unsafe_fn)]

use std::{ffi::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, madvise};

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, HEADER_SIZE, OX_CURRENT_STAMP, OxHeader, OxidallocError,
    slab::{
        NUM_SIZE_CLASSES, SIZE_CLASSES, get_size_4096_class,
        global::{GLOBAL, GlobalHandler, MAX_NUMA_NODES},
        thread_local::ThreadLocalEngine,
    },
    trim::{
        TimeDecay,
        thread::{GLOBAL_DECAY, LAST_PRESSURE_CHECK},
    },
    va::{bootstrap::SHUTDOWN, is_ours},
};

pub struct GTrim;

impl GTrim {
    unsafe fn pop_from_global(
        &self,
        class: usize,
        numa_node_id: usize,
        thread: &ThreadLocalEngine,
    ) -> (*mut OxHeader, usize) {
        let global_cache = GlobalHandler.pop_from_global_local(numa_node_id, class, 16);

        if global_cache.is_null() {
            return (null_mut(), 0);
        }

        let mut block = global_cache;
        let mut real = 1;

        while real < 16 && !(*block).next.is_null() && is_ours((*block).next as usize, Some(thread))
        {
            if (*block).in_use == 1 {
                OxidallocError::MemoryCorruption.log_and_abort(
                    block as *mut c_void,
                    "Find in_use block during PThread Trim / Memory Corruption",
                    None,
                );
            }

            block = (*block).next;
            real += 1;
        }
        (*block).next = null_mut();

        (global_cache, real)
    }

    pub unsafe fn trim(&self, pad: usize) -> (i32, usize) {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return (0, 0);
        }
        let pressure = LAST_PRESSURE_CHECK.load(Ordering::Relaxed);
        let force_trim = pressure > 90;
        let thread_engine = ThreadLocalEngine::get_or_init();

        let mut avg = 0;
        let mut total = 0;
        let mut total_freed = 0;
        let timing = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed);
        let class_4096 = get_size_4096_class();

        for numa in 0..MAX_NUMA_NODES {
            for class in class_4096..NUM_SIZE_CLASSES {
                if total_freed >= pad && pad != 0 {
                    return (1, total_freed);
                }

                let class_usage = GLOBAL[numa].usage[class].load(Ordering::Relaxed);
                let mut to_trim = null_mut();

                for _ in 0..class_usage / 16 {
                    let (cache, size) = self.pop_from_global(class, numa, thread_engine);
                    if cache.is_null() {
                        break;
                    }

                    let mut block = cache;
                    let mut to_push = null_mut();

                    for _ in 0..size {
                        let next = (*block).next;
                        let life_time = OX_CURRENT_STAMP
                            .load(Ordering::Relaxed)
                            .saturating_sub((*block).life_time);

                        if life_time != 0 {
                            avg += life_time;
                            total += 1;
                        }

                        if life_time > timing {
                            (*block).next = to_trim;
                            to_trim = block;
                        } else {
                            (*block).next = to_push;
                            to_push = block;
                        }

                        block = next;
                    }

                    if to_push.is_null() {
                        continue;
                    }

                    let mut block = to_push;
                    let mut real = 1;

                    while real < 16
                        && !(*block).next.is_null()
                        && is_ours((*block).next as usize, None)
                    {
                        block = (*block).next;
                        real += 1;
                    }
                    (*block).next = null_mut();

                    GlobalHandler.push_to_global(class, numa, to_push, block, real);
                }

                while !to_trim.is_null() {
                    let next = (*to_trim).next;

                    if (total_freed <= pad || pad == 0) || force_trim {
                        self.release_memory(to_trim, SIZE_CLASSES[class]);
                        (*to_trim).used_before = 0;
                        total_freed += SIZE_CLASSES[class];
                    }

                    GlobalHandler.push_to_global(class, numa, to_trim, to_trim, 1);

                    to_trim = next;
                }
            }
        }

        if total > 0 {
            let avg = (avg / total).max(100).min(10000);
            AVERAGE_BLOCK_TIMES_GLOBAL.store(avg, Ordering::Relaxed);
            GLOBAL_DECAY.swap(TimeDecay::decide_on(avg) as u8, Ordering::AcqRel);
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
