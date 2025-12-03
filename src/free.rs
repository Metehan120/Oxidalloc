use std::{
    os::raw::c_void,
    process::abort,
    sync::atomic::{AtomicUsize, Ordering},
};

use libc::{madvise, munmap};

use crate::{
    HEADER_SIZE, OxHeader, TOTAL_OPS,
    internals::{AllocationHelper, MAGIC, VA_END, VA_START},
    thread_local::ThreadLocalEngine,
    trim::Trim,
};

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
pub static TOTAL_FREE: AtomicUsize = AtomicUsize::new(0);

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

        if TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 10000 == 0 {
            Trim.trim(thread);
        }
    }
}
