use std::{
    hint::unlikely,
    os::raw::{c_int, c_void},
};

use crate::{
    HEADER_SIZE, OX_ALIGN_TAG, OxHeader, OxidallocError,
    inner::{
        alloc::{OFFSET_SIZE, TAG_SIZE, alloc_inner},
        fallback::malloc_usable_size_fallback,
    },
    internals::{hashmap::BIG_ALLOC_MAP, size_t},
    slab::SIZE_CLASSES,
    trim::gtrim::GTrim,
    va::is_ours,
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: size_t) -> *mut c_void {
    alloc_inner(size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> size_t {
    if ptr.is_null() {
        return 0;
    }

    if !is_ours(ptr as usize) {
        return malloc_usable_size_fallback(ptr);
    }

    let mut raw_ptr = ptr;
    let mut offset: usize = 0;
    let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;

    if unlikely(std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG) {
        let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;
        let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
        if is_ours(presumed_original_ptr as usize) {
            raw_ptr = presumed_original_ptr;
            offset = (ptr as usize).wrapping_sub(raw_ptr as usize);
        }
    }

    let header = (raw_ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

    let class = (*header).class as usize;
    let raw_usable = if class == 100 {
        let payload_size = BIG_ALLOC_MAP
            .get(header as usize)
            .map(|meta| meta.size)
            .unwrap_or_else(|| {
                OxidallocError::AttackOrCorruption.log_and_abort(
                    header as *mut c_void,
                    "Missing big allocation metadata during malloc_usable_size",
                    None,
                )
            });
        payload_size
    } else {
        SIZE_CLASSES[class]
    };

    raw_usable.saturating_sub(offset) as size_t
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_trim(pad: size_t) -> c_int {
    GTrim.trim(pad).0
}
