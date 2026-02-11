use std::{ffi::c_void, ptr::null_mut, sync::atomic::Ordering};

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, FREED_MAGIC, HEADER_SIZE, OX_CURRENT_STAMP, OxHeader,
    OxidallocError,
    big_allocation::trim_big_allocations,
    slab::{NUM_SIZE_CLASSES, SIZE_CLASSES, get_size_4096_class, interconnect::ICC},
    sys::memory_system::{MadviseFlags, madvise},
    trim::{
        TimeDecay,
        thread::{GLOBAL_DECAY, LAST_PRESSURE_CHECK},
    },
    va::is_ours,
};

pub struct GTrim;

impl GTrim {
    unsafe fn pop_from_global(&self, class: usize, need_pushed: bool) -> (*mut OxHeader, usize) {
        let global_cache = ICC.try_pop(class, 16, need_pushed);

        if global_cache.is_null() {
            return (null_mut(), 0);
        }

        let mut block = global_cache;
        let mut real = 1;

        while real < 16 && !(*block).next.is_null() && is_ours((*block).next as usize) {
            if (*block).magic != FREED_MAGIC {
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

        #[cfg(feature = "hardened-malloc")]
        {
            if (*block).magic != FREED_MAGIC {
                OxidallocError::MemoryCorruption.log_and_abort(
                    block as *mut c_void,
                    "Find in_use block during PThread Trim / Memory Corruption",
                    None,
                );
            }
        }

        (global_cache, real)
    }

    pub unsafe fn trim(&self, pad: usize) -> (i32, usize) {
        let pressure = LAST_PRESSURE_CHECK.load(Ordering::Relaxed);
        let force_trim = pressure > 90;

        let mut avg: u32 = 0;
        let mut total = 0;
        let mut total_freed = 0;
        total_freed += trim_big_allocations();

        let timing = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed) as u32;
        let class_4096 = get_size_4096_class();

        for class in class_4096..NUM_SIZE_CLASSES {
            if total_freed >= pad && pad != 0 {
                return (1, total_freed);
            }

            let class_usage = ICC.get_size(class);
            let mut to_trim = null_mut();
            let mut total_loop = 0;

            let mut need_pushed = false;
            for _ in 0..class_usage / 16 {
                let (cache, size) = self.pop_from_global(class, need_pushed);
                if cache.is_null() {
                    if total_loop < (class_usage / 16) && !need_pushed {
                        need_pushed = true;
                        continue;
                    }

                    break;
                }

                let mut block = cache;
                let mut to_push = null_mut();

                for _ in 0..size {
                    let next = (*block).next;
                    let life_time = OX_CURRENT_STAMP.saturating_sub((*block).life_time);

                    if life_time != 0 {
                        avg = avg.saturating_add(life_time);
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

                let mut tail = to_push;
                let mut real = 1;

                while real < 16 && !(*tail).next.is_null() && is_ours((*tail).next as usize) {
                    tail = (*tail).next;
                    real += 1;
                }
                (*tail).next = null_mut();

                ICC.try_push(class, to_push, tail, real, true, false);

                total_loop += 1;
            }

            while !to_trim.is_null() {
                let next = (*to_trim).next;

                let mut trimmed = false;
                if (total_freed <= pad || pad == 0) || force_trim {
                    self.release_memory(to_trim, SIZE_CLASSES[class]);
                    total_freed += SIZE_CLASSES[class];
                    trimmed = true;
                }

                ICC.try_push(class, to_trim, to_trim, 1, true, trimmed);

                to_trim = next;
            }
        }

        if total > 0 {
            let avg = (avg / total).max(1).min(10);
            AVERAGE_BLOCK_TIMES_GLOBAL.store(avg as usize, Ordering::Relaxed);
            GLOBAL_DECAY.swap(TimeDecay::decide_on(avg as usize) as u8, Ordering::AcqRel);
        }

        if total_freed >= pad {
            (1, total_freed)
        } else {
            (0, total_freed)
        }
    }

    #[inline]
    unsafe fn release_memory(&self, header_ptr: *mut OxHeader, size: usize) {
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

        let _ = madvise(page_start as *mut c_void, length, MadviseFlags::DONTNEED);
    }
}
