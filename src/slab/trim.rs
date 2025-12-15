use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    HEADER_SIZE, OX_CURRENT_STAMP, OxHeader, OxidallocError, TOTAL_ALLOCATED,
    slab::{
        SIZE_CLASSES,
        global::{GLOBAL, GLOBAL_USAGE, GlobalHandler},
        thread_local::ThreadLocalEngine,
    },
    va::bitmap::VA_MAP,
};

pub static IS_TRIMMING: AtomicBool = AtomicBool::new(false);

pub struct Trim;

// Good enough, dont touch it.
impl Trim {
    pub fn pop_from_thread(&self, cache: &ThreadLocalEngine, class: usize) -> *mut OxHeader {
        unsafe {
            let cache_mem = cache.pop_from_thread(class);

            if cache_mem.is_null() {
                return null_mut();
            }

            (*cache_mem).next = null_mut();

            if (*cache_mem).in_use == 1 {
                OxidallocError::MemoryCorruption.log_and_abort(
                    cache_mem as *mut c_void,
                    "In-use block found in free list during trim.",
                    None,
                );
            }

            cache_mem
        }
    }

    pub fn trim(&self, cache: &ThreadLocalEngine) {
        unsafe {
            for class in (0..cache.cache.len()).rev() {
                let mut to_trim = null_mut();
                for _ in 0..cache.usages[class].load(Ordering::Relaxed) {
                    let cache_mem = self.pop_from_thread(cache, class);

                    if cache_mem.is_null() {
                        continue;
                    }

                    let time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*cache_mem).life_time);

                    if time > 2 {
                        cache.usages[class].fetch_sub(1, Ordering::Relaxed);
                        (*cache_mem).flag = 2;

                        (*cache_mem).next = to_trim;
                        to_trim = cache_mem;
                        continue;
                    }

                    cache.push_to_thread(class, cache_mem);
                }

                while !to_trim.is_null() {
                    let next = (*to_trim).next;
                    (*to_trim).next = null_mut();

                    let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);
                    if self.release_memory(to_trim, SIZE_CLASSES[class]) {
                        TOTAL_ALLOCATED.fetch_sub(1, Ordering::Relaxed);
                        (*to_trim).life_time = current;
                    } else {
                        GlobalHandler.push_to_global(class, to_trim, to_trim, 1);
                        GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);
                    }

                    to_trim = next;
                }

                for _ in 0..cache.usages[class].load(Ordering::Relaxed) {
                    let cache_mem = self.pop_from_thread(cache, class);

                    if cache_mem.is_null() {
                        break;
                    }

                    let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);
                    let time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*cache_mem).life_time);

                    self.will_need(cache_mem as *mut c_void, SIZE_CLASSES[class]);

                    if time > 1 {
                        cache.usages[class].fetch_sub(1, Ordering::Relaxed);
                        GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);
                        (*cache_mem).life_time = current;

                        GlobalHandler.push_to_global(class, cache_mem, cache_mem, 1);
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
                let mut to_trim = null_mut();
                for _ in 0..GLOBAL_USAGE[class].load(Ordering::Relaxed) {
                    let global_mem = GlobalHandler.pop_batch_from_global(class, 1);

                    if global_mem.is_null() {
                        break;
                    }

                    (*global_mem).next = null_mut();

                    let time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*global_mem).life_time);

                    if (*global_mem).in_use == 1 {
                        OxidallocError::MemoryCorruption.log_and_abort(
                            global_mem as *mut c_void,
                            "In-use block found in free list during trim.",
                            None,
                        );
                    }

                    if time > 2 {
                        (*global_mem).flag = 2;

                        (*global_mem).next = to_trim;
                        to_trim = global_mem;
                        continue;
                    }

                    self.will_need(global_mem as *mut c_void, SIZE_CLASSES[class]);

                    GlobalHandler.push_to_global(class, global_mem, global_mem, 1);
                    GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);
                }

                while !to_trim.is_null() {
                    let next = (*to_trim).next;
                    (*to_trim).next = null_mut();

                    if self.release_memory(to_trim, SIZE_CLASSES[class]) {
                        TOTAL_ALLOCATED.fetch_sub(1, Ordering::Relaxed);
                    } else {
                        GlobalHandler.push_to_global(class, to_trim, to_trim, 1);
                        GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);
                    }

                    to_trim = next;
                }
            }

            IS_TRIMMING.store(false, Ordering::Release);
        }
    }

    #[inline]
    fn will_need(&self, header_ptr: *mut c_void, size: usize) {
        unsafe {
            const PAGE_SIZE: usize = 4096;
            const PAGE_MASK: usize = !(PAGE_SIZE - 1);

            let alloc_start = header_ptr as usize;
            let alloc_end = alloc_start + HEADER_SIZE + size;

            let page_start = (alloc_start + PAGE_SIZE - 1) & PAGE_MASK;
            let page_end = alloc_end & PAGE_MASK;

            if page_start < page_end {
                let length = page_end - page_start;

                libc::madvise(page_start as *mut c_void, length, libc::MADV_WILLNEED);
            }
        }
    }

    #[inline]
    fn release_memory(&self, header_ptr: *mut OxHeader, size: usize) -> bool {
        unsafe {
            const PAGE_SIZE: usize = 4096;
            const PAGE_MASK: usize = !(PAGE_SIZE - 1);

            let alloc_start = header_ptr as usize;
            let alloc_end = alloc_start + HEADER_SIZE + size;

            if (alloc_start & (PAGE_SIZE - 1)) != 0 || (alloc_end & (PAGE_SIZE - 1)) != 0 {
                return false;
            }

            let page_start = alloc_start & PAGE_MASK;
            let page_end = alloc_end & PAGE_MASK;

            if page_start < page_end {
                let length = page_end - page_start;

                if libc::madvise(page_start as *mut c_void, length, libc::MADV_DONTNEED) != 0 {
                    return false;
                };

                if length > 1024 * 64 {
                    VA_MAP.free(page_start as usize, length)
                }

                return true;
            }

            false
        }
    }
}
