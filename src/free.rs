use std::{os::raw::c_void, process::abort, sync::atomic::Ordering};

use libc::{madvise, munmap};

use crate::{
    HEADER_SIZE, OxHeader, TOTAL_ALLOCATED, TOTAL_IN_USE, TOTAL_OPS,
    internals::{AllocationHelper, MAGIC, VA_END, VA_START},
    thread_local::ThreadLocalEngine,
    trim::Trim,
};

#[inline(always)]
pub fn is_ours(ptr: *mut c_void) -> bool {
    let start = VA_START.load(Ordering::Acquire);
    let end = VA_END.load(Ordering::Acquire);

    if start == 0 || end == 0 || start >= end {
        return false;
    }

    let addr = ptr as usize;
    addr >= start && addr < end
}

const OFFSET_SIZE: usize = size_of::<usize>();

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    unsafe {
        if ptr.is_null() {
            return;
        }

        let mut header_search_ptr = ptr;
        let presumed_original_ptr_loc =
            (ptr as usize).wrapping_sub(OFFSET_SIZE) as *mut *mut c_void;
        let presumed_original_ptr = *presumed_original_ptr_loc;

        if !presumed_original_ptr.is_null() && is_ours(presumed_original_ptr) {
            header_search_ptr = presumed_original_ptr;
        }

        if !is_ours(header_search_ptr) {
            return;
        }

        let header_addr = (header_search_ptr as usize).wrapping_sub(HEADER_SIZE);
        let header = header_addr as *mut OxHeader;

        if !is_ours(header as *mut c_void) {
            return;
        }

        if header_addr % 4096 > 4096 - HEADER_SIZE {
            return;
        }

        let magic_val = std::ptr::read_volatile(&(*header).magic);

        if magic_val != MAGIC && magic_val != 0 {
            eprintln!(
                "Double Free or Memory Corruption | Undefined Behaviour ptr={:p}",
                header
            );
            abort()
        }

        let size = (*header).size;
        let class = match AllocationHelper.match_size_class(size as usize) {
            Some(class) => class,
            None => {
                (*header).magic = 0;

                if munmap(header as *mut c_void, HEADER_SIZE + size as usize) != 0 {
                    madvise(
                        header as *mut c_void,
                        HEADER_SIZE + size as usize,
                        libc::MADV_DONTNEED,
                    );
                }
                return;
            }
        };

        (*header).magic = 0;

        let thread = ThreadLocalEngine::get_or_init();
        thread.push_to_thread(class, header);
        thread.usages[class].fetch_add(1, Ordering::Relaxed);

        TOTAL_IN_USE.fetch_sub(1, Ordering::Relaxed);

        trim(thread);
    }
}

#[inline(always)]
pub fn trim(engine: &ThreadLocalEngine) {
    let total = TOTAL_OPS.load(Ordering::Relaxed);

    let total_allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);
    let total_in_use = TOTAL_IN_USE.load(Ordering::Relaxed);

    let in_use_percentage = if total_allocated > 0 {
        (total_in_use * 100) / total_allocated
    } else {
        0
    };

    if total % 20000 == 0 && in_use_percentage < 50 || in_use_percentage < 25 {
        Trim.trim_global();
    } else if total % 10000 == 0 && in_use_percentage < 50 {
        Trim.trim(engine);
    }
}
