#![allow(unsafe_op_in_unsafe_fn)]

use std::{ffi::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, madvise};

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, HEADER_SIZE, OX_CURRENT_STAMP, OxHeader, OxidallocError,
    slab::{
        ITERATIONS, SIZE_CLASSES,
        global::{GLOBAL_USAGE, GlobalHandler},
        pack_header, unpack_header,
    },
    va::{
        bootstrap::{GLOBAL_RANDOM, SHUTDOWN},
        va_helper::is_ours,
    },
};

pub struct GTrim;

impl GTrim {
    unsafe fn pop_from_global(&self, class: usize) -> (*mut OxHeader, usize) {
        let global_cache = GlobalHandler.pop_batch_from_global(class, 16);

        if global_cache.is_null() {
            return (null_mut(), 0);
        }

        let mut block = global_cache;
        let mut real = 1;

        while real < 16
            && !unpack_header((*block).next, GLOBAL_RANDOM).is_null()
            && is_ours(unpack_header((*block).next, GLOBAL_RANDOM) as usize)
        {
            if (*block).in_use == 1 {
                OxidallocError::MemoryCorruption.log_and_abort(
                    block as *mut c_void,
                    "Find in_use block during PThread Trim / Memory Corruption",
                    None,
                );
            }

            block = unpack_header((*block).next, GLOBAL_RANDOM);
            real += 1;
        }

        (*block).next = pack_header(null_mut(), GLOBAL_RANDOM);

        (global_cache, real)
    }

    pub unsafe fn trim(&self, pad: usize) -> (i32, usize) {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return (0, 0);
        }

        let mut avg = 0;
        let mut total = 0;
        let mut total_freed = 0;
        let timing = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed);

        for class in 9..ITERATIONS.len() {
            if total_freed >= pad && pad != 0 {
                return (1, total_freed);
            }

            let class_usage = GLOBAL_USAGE[class].load(Ordering::Relaxed);
            let mut to_trim = null_mut();

            for _ in 0..class_usage / 16 {
                let (cache, size) = self.pop_from_global(class);
                if cache.is_null() {
                    break;
                }

                let mut block = cache;
                let mut to_push = null_mut();

                for _ in 0..size {
                    let next = unpack_header((*block).next, GLOBAL_RANDOM);
                    let life_time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*block).life_time);

                    if life_time != 0 {
                        avg += life_time;
                        total += 1;
                    }

                    if life_time > timing {
                        (*block).next = pack_header(to_trim, GLOBAL_RANDOM);
                        to_trim = block;
                    } else {
                        (*block).next = pack_header(to_push, GLOBAL_RANDOM);
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
                    && !unpack_header((*block).next, GLOBAL_RANDOM).is_null()
                    && is_ours(unpack_header((*block).next, GLOBAL_RANDOM) as usize)
                {
                    block = unpack_header((*block).next, GLOBAL_RANDOM);
                    real += 1;
                }
                (*block).next = pack_header(null_mut(), GLOBAL_RANDOM);

                GlobalHandler.push_to_global(class, to_push, block, real);
            }

            while !to_trim.is_null() {
                let next = unpack_header((*to_trim).next, GLOBAL_RANDOM);

                if total_freed <= pad || pad == 0 {
                    self.release_memory(to_trim, SIZE_CLASSES[class]);
                    total_freed += SIZE_CLASSES[class];
                }

                GlobalHandler.push_to_global(class, to_trim, to_trim, 1);

                to_trim = next;
            }
        }

        if total > 0 {
            AVERAGE_BLOCK_TIMES_GLOBAL.store((avg / total).max(100).min(3000), Ordering::Relaxed);
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
