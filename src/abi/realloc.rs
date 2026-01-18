#![allow(unsafe_op_in_unsafe_fn)]

use libc::{__errno_location, size_t};
use rustix::mm::{MremapFlags, mremap};
use std::{os::raw::c_void, ptr::null_mut};

use crate::{
    HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OxHeader,
    abi::{fallback::realloc_fallback, free::free, malloc::malloc},
    slab::match_size_class,
    va::{align_to, bitmap::VA_MAP, is_ours},
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

// TODO: Better realloc implementation
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void {
    if ptr.is_null() {
        return malloc(new_size);
    }

    if !is_ours(ptr as usize) {
        return realloc_fallback(ptr, new_size);
    }

    if new_size == 0 {
        free(ptr);
        return malloc(1);
    }

    let mut raw_ptr = ptr;
    let mut offset: usize = 0;
    let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
    let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;

    // Found offset so we can calculate the original pointer
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

    let old_class = match_size_class(raw_capacity);
    let new_class = match_size_class(new_size);

    if let (Some(old), Some(new)) = (old_class, new_class) {
        if old == new {
            return ptr;
        }
    }

    if old_class.is_none() {
        let old_total = align_to(raw_capacity + HEADER_SIZE, 4096);
        let new_total = align_to(new_size + HEADER_SIZE, 4096);

        if new_total == old_total {
            (*header).size = new_size;
            return ptr;
        }

        if new_total < old_total {
            let _ = mremap(
                header as *mut c_void,
                old_total,
                new_total,
                MremapFlags::empty(),
            );

            VA_MAP.free((header as usize) + new_total, old_total - new_total);

            (*header).size = new_size;
            return ptr;
        }

        if let Some(actual_new_va_size) =
            VA_MAP.realloc_inplace(header as usize, old_total, new_total)
        {
            let resmap_res = mremap(
                header as *mut c_void,
                old_total,
                actual_new_va_size,
                MremapFlags::empty(),
            );

            if resmap_res.is_ok() {
                (*header).size = new_size;
                return ptr;
            } else {
                VA_MAP.free(
                    (header as usize) + old_total,
                    actual_new_va_size - old_total,
                );
            }
        }
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn reallocarray(
    ptr: *mut c_void,
    nmemb: size_t,
    size: size_t,
) -> *mut c_void {
    let total_size = match nmemb.checked_mul(size) {
        Some(s) => s,
        None => {
            if let Some(errno_ptr) = __errno_location().as_mut() {
                *errno_ptr = libc::ENOMEM;
            }
            return null_mut();
        }
    };

    realloc(ptr, total_size)
}
