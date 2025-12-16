use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use libc::size_t;

use crate::{
    MAGIC, OX_ALIGN_TAG, OxHeader, TOTAL_OPS,
    abi::{free::free, malloc::malloc},
    va::{bootstrap::VA_LEN, va_helper::is_ours},
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

// TODO: mremap logic
#[unsafe(no_mangle)]
pub extern "C" fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void {
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);

    if ptr.is_null() {
        return malloc(new_size);
    }

    if !is_ours(ptr as usize) {
        return null_mut();
    }

    TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if new_size > VA_LEN.load(Ordering::Relaxed) {
        return null_mut();
    }

    if new_size == 0 {
        free(ptr);
        return malloc(1);
    }

    if new_size > VA_LEN.load(Ordering::Relaxed) {
        return null_mut();
    }

    unsafe {
        let mut raw_ptr = ptr;
        let mut offset: usize = 0;
        let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
        let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;

        if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
            let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
            if is_ours(presumed_original_ptr as usize) {
                raw_ptr = presumed_original_ptr;
                offset = (ptr as usize).wrapping_sub(raw_ptr as usize);
            }
        }

        let header = (raw_ptr as *mut OxHeader).sub(1);

        if (*header).magic != MAGIC && (*header).magic != 0 {
            return null_mut();
        }

        let raw_capacity = (*header).size as usize;
        let old_capacity = raw_capacity.saturating_sub(offset);

        if new_size <= old_capacity {
            return ptr;
        }

        let new_ptr = malloc(new_size);
        if new_ptr.is_null() {
            return std::ptr::null_mut();
        }

        std::ptr::copy_nonoverlapping(
            ptr as *const u8,
            new_ptr as *mut u8,
            old_capacity.min(new_size),
        );

        free(ptr);
        new_ptr
    }
}
