#![allow(unsafe_op_in_unsafe_fn)]

use std::{ffi::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, madvise};

use crate::{
    HEADER_SIZE, OX_CURRENT_STAMP, OxHeader, OxidallocError,
    slab::{
        ITERATIONS, SIZE_CLASSES,
        global::{GLOBAL_USAGE, GlobalHandler},
    },
    va::va_helper::is_ours,
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

        while real < 16 && !(*block).next.is_null() && is_ours((*block).next as usize) {
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

    pub unsafe fn trim(&self) {
        for class in 0..ITERATIONS.len() {
            let class_usage = GLOBAL_USAGE[class].load(Ordering::Relaxed);
            let mut to_trim = null_mut();

            for _ in 0..class_usage / 16 {
                let (cache, size) = self.pop_from_global(class);
                if cache.is_null() {
                    break;
                }

                let mut block = cache;
                let mut to_push = null_mut();

                for _ in 9..size {
                    let next = (*block).next;
                    let life_time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*block).life_time);

                    if life_time > 25000 {
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

                while real < 16 && !(*block).next.is_null() && is_ours((*block).next as usize) {
                    block = (*block).next;
                    real += 1;
                }
                (*block).next = null_mut();

                GlobalHandler.push_to_global(class, to_push, block, real);
            }

            while !to_trim.is_null() {
                let next = (*to_trim).next;

                if !self.release_memory(to_trim, SIZE_CLASSES[class]) {
                    GlobalHandler.push_to_global(class, to_trim, to_trim, 1);
                }

                to_trim = next;
            }
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

            madvise(page_start as *mut c_void, length, Advice::LinuxDontNeed).is_ok()
        }
    }
}
