use std::{os::raw::c_void, ptr::null_mut};

use crate::{
    inner::alloc::{OFFSET_SIZE, TAG_SIZE, alloc_inner},
    sys::{EINVAL, NOMEM},
};

#[inline(always)]
pub unsafe fn align_inner(memptr: *mut *mut c_void, alignment: usize, size: usize) -> i32 {
    if memptr.is_null() {
        return EINVAL;
    }

    let min = size_of::<*mut c_void>();
    if alignment < min || !alignment.is_power_of_two() {
        return EINVAL;
    }

    let Some(total_requested) = size
        .checked_add(alignment)
        .and_then(|v| v.checked_add(TAG_SIZE))
    else {
        return NOMEM;
    };

    let raw = alloc_inner(total_requested);
    if raw.is_null() {
        return NOMEM;
    }

    let addr = raw as usize;
    let start_search = addr.saturating_add(TAG_SIZE);
    let aligned = (start_search + alignment - 1) & !(alignment - 1);

    let tag_location = aligned.saturating_sub(TAG_SIZE) as *mut usize;
    let original_ptr_location = aligned.saturating_sub(OFFSET_SIZE) as *mut usize;
    *tag_location = crate::OX_ALIGN_TAG;
    *original_ptr_location = raw as usize;

    *memptr = aligned as *mut c_void;
    0
}

#[inline(always)]
pub unsafe fn memalign_inner(alignment: usize, size: usize) -> *mut c_void {
    let align = alignment.max(size_of::<*mut c_void>());
    if !align.is_power_of_two() {
        return null_mut();
    }

    let target_size = (size.max(align) + align - 1) & align.wrapping_neg();

    alloc_inner(target_size)
}
