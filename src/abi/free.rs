#![allow(unsafe_op_in_unsafe_fn)]

use libc::size_t;

use crate::{
    HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OX_CURRENT_STAMP, OxHeader, OxidallocError,
    abi::{fallback::free_fallback, malloc::TOTAL_MALLOC_FREE},
    big_allocation::big_free,
    slab::{match_size_class, thread_local::ThreadLocalEngine},
    va::is_ours,
};
use std::{alloc::Layout, os::raw::c_void, ptr::read_volatile, sync::atomic::Ordering};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

#[unsafe(no_mangle)]
// If we seperate free nothing will change much, free can stay naked for now
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    if TOTAL_MALLOC_FREE.load(Ordering::Relaxed) < 256 {
        TOTAL_MALLOC_FREE.fetch_add(1, Ordering::Relaxed);
    }

    if ptr.is_null() {
        return;
    }

    let thread = ThreadLocalEngine::get_or_init();
    if !is_ours(ptr as usize, Some(thread)) {
        free_fallback(ptr);
        return;
    }

    let mut header_search_ptr = ptr;
    let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
    let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;
    if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
        let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
        if is_ours(presumed_original_ptr as usize, Some(thread)) {
            header_search_ptr = presumed_original_ptr;
        }
    }

    let header_addr = (header_search_ptr as usize).wrapping_sub(HEADER_SIZE);
    let header = header_addr as *mut OxHeader;
    let magic = read_volatile(&(*header).magic);
    let in_use = read_volatile(&(*header).in_use);

    if magic != MAGIC && magic != 0 {
        OxidallocError::MemoryCorruption.log_and_abort(
            header as *mut c_void,
            "Possibly Double Free",
            None,
        );
    }

    if in_use == 0 {
        OxidallocError::DoubleFree.log_and_abort(
            header as *mut c_void,
            "Pointer is tagged as in_use",
            None,
        );
    }

    let size = (*header).size as usize;
    match Layout::array::<u8>(size) {
        Ok(layout) => free_inner(&layout, header, thread, header_search_ptr),
        Err(_) => return,
    };
}

#[inline(always)]
unsafe fn free_inner(
    layout: &Layout,
    header: *mut OxHeader,
    thread: &ThreadLocalEngine,
    header_search_ptr: *mut c_void,
) {
    let class = match match_size_class(layout.size()) {
        Some(class) => class,
        None => {
            big_free(header_search_ptr as *mut OxHeader);
            return;
        }
    };

    let stamp = OX_CURRENT_STAMP.load(Ordering::Relaxed);

    (*header).in_use = 0;
    (*header).magic = 0;
    (*header).life_time = stamp;

    thread.push_to_thread(class, header);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_sized(ptr: *mut c_void, _: size_t) {
    free(ptr);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_aligned_sized(ptr: *mut c_void, _: size_t, _: size_t) {
    free(ptr);
}
