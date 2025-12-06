use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    FLAG_FREED, HEADER_SIZE, OX_CURRENT_STAMP, OxidallocError,
    global::{GLOBAL, GLOBAL_USAGE, GlobalHandler},
    internals::SIZE_CLASSES,
    thread_local::ThreadLocalEngine,
};

pub static IS_TRIMMING: AtomicBool = AtomicBool::new(false);

pub struct Trim;

impl Trim {
    pub fn trim(&self, cache: &ThreadLocalEngine) {
        unsafe {
            for class in (0..cache.cache.len()).rev() {
                for _ in 0..cache.usages[class].load(Ordering::Relaxed) / 2 {
                    let cache_mem = cache.pop_from_thread(class);

                    if cache_mem.is_null() {
                        break;
                    }

                    (*cache_mem).next = null_mut();

                    let time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*cache_mem).life_time);

                    if (*cache_mem).in_use.load(Ordering::Relaxed) == 1 {
                        OxidallocError::MemoryCorruption.log_and_abort(
                            cache_mem as *mut c_void,
                            "In-use block found in free list during trim.",
                        );
                    }

                    if time > 2 {
                        cache.usages[class].fetch_sub(1, Ordering::Relaxed);
                        (*cache_mem).life_time = 0;
                        (*cache_mem).flag = FLAG_FREED;

                        self.release_memory(cache_mem, SIZE_CLASSES[class]);
                        continue;
                    }

                    if time > 1 {
                        cache.usages[class].fetch_sub(1, Ordering::Relaxed);
                        GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);
                        (*cache_mem).life_time = 0;

                        GlobalHandler.push_to_global(class, cache_mem, cache_mem);
                        continue;
                    }

                    cache.push_to_thread(class, cache_mem);
                }
            }
        }
    }

    pub fn trim_global(&self) {
        unsafe {
            if IS_TRIMMING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                return;
            }

            for class in (0..GLOBAL.len()).rev() {
                for _ in 0..GLOBAL_USAGE[class].load(Ordering::Relaxed) / 2 {
                    let global_mem = GlobalHandler.pop_batch_from_global(class, 1);

                    if global_mem.is_null() {
                        break;
                    }

                    (*global_mem).next = null_mut();

                    let time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*global_mem).life_time);

                    if (*global_mem).in_use.load(Ordering::Relaxed) == 1 {
                        OxidallocError::MemoryCorruption.log_and_abort(
                            global_mem as *mut c_void,
                            "In-use block found in free list during trim.",
                        );
                    }

                    if time > 2 {
                        (*global_mem).life_time = 0;
                        (*global_mem).flag = FLAG_FREED;

                        self.release_memory(global_mem, SIZE_CLASSES[class]);
                        continue;
                    }

                    GlobalHandler.push_to_global(class, global_mem, global_mem);
                    GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);
                }
            }

            IS_TRIMMING.store(false, Ordering::Release);
        }
    }

    #[inline]
    fn release_memory(&self, header_ptr: *mut crate::OxHeader, size: usize) {
        unsafe {
            let payload_start = (header_ptr as usize) + HEADER_SIZE;

            let payload_end = payload_start + size;

            const PAGE_SIZE: usize = 4096;
            let aligned_start = (payload_start + (PAGE_SIZE - 1)) & !(PAGE_SIZE - 1);

            if aligned_start < payload_end {
                let length = payload_end - aligned_start;

                if length >= PAGE_SIZE {
                    let aligned_length = length & !(PAGE_SIZE - 1);

                    if aligned_length > 0 {
                        libc::madvise(
                            aligned_start as *mut c_void,
                            aligned_length,
                            libc::MADV_DONTNEED,
                        );
                    }
                }
            }
        }
    }
}
