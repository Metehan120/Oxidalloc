use crate::{
    abi::calloc::CALLOC,
    inner::realloc::realloc_inner,
    internals::{__errno_location, size_t},
    sys::NOMEM,
};
use std::{os::raw::c_void, ptr::null_mut};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void {
    realloc_inner(ptr, new_size)
}

static REALLOC: unsafe extern "C" fn(*mut c_void, size_t) -> *mut c_void = realloc;

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
                *errno_ptr = NOMEM;
            }
            return null_mut();
        }
    };

    (REALLOC)(ptr, total_size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn recallocarray(
    ptr: *mut c_void,
    oldnmemb: size_t,
    newnmemb: size_t,
    size: size_t,
) -> *mut c_void {
    if ptr.is_null() {
        return (CALLOC)(newnmemb, size);
    }

    let new_size = match newnmemb.checked_mul(size) {
        Some(s) => s,
        None => {
            if let Some(errno_ptr) = __errno_location().as_mut() {
                *errno_ptr = NOMEM;
            }
            return null_mut();
        }
    };

    let old_size = match oldnmemb.checked_mul(size) {
        Some(s) => s,
        None => 0,
    };

    if new_size <= old_size {
        return (REALLOC)(ptr, new_size);
    }

    let new_ptr = (REALLOC)(ptr, new_size);
    if !new_ptr.is_null() {
        let grow_size = new_size - old_size;
        std::ptr::write_bytes((new_ptr as *mut u8).add(old_size), 0, grow_size);
    }

    new_ptr
}
