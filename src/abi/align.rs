use std::{
    os::raw::{c_int, c_void},
    ptr::null_mut,
};

use crate::{
    abi::malloc::malloc,
    internals::size_t,
    sys::{EINVAL, NOMEM},
    va::align_to,
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_memalign(
    memptr: *mut *mut c_void,
    alignment: usize,
    size: usize,
) -> c_int {
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

    let mut raw = malloc(total_requested);
    if raw.is_null() {
        let malloc = malloc(total_requested);
        if !malloc.is_null() {
            raw = malloc;
        } else {
            return NOMEM;
        };
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memalign(alignment: size_t, size: size_t) -> *mut c_void {
    let mut ptr: *mut c_void = null_mut();
    let adjusted_alignment = if alignment < 8 { 8 } else { alignment };

    if !adjusted_alignment.is_power_of_two() {
        return null_mut();
    }

    if posix_memalign(&mut ptr, adjusted_alignment, size) == 0 {
        ptr
    } else {
        null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aligned_alloc(alignment: size_t, size: size_t) -> *mut c_void {
    if alignment == 0 || !alignment.is_power_of_two() {
        return null_mut();
    }
    memalign(alignment, size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valloc(size: size_t) -> *mut c_void {
    memalign(4096, size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pvalloc(size: size_t) -> *mut c_void {
    let page_size = 4096;
    let rounded_size = if size == 0 {
        page_size
    } else {
        align_to(size, page_size)
    };

    memalign(page_size, rounded_size)
}
