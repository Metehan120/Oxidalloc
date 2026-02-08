use std::{
    os::raw::{c_int, c_void},
    ptr::null_mut,
};

use crate::{
    inner::memalign::{align_inner, memalign_inner},
    internals::size_t,
    va::align_to,
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_memalign(
    memptr: *mut *mut c_void,
    alignment: usize,
    size: usize,
) -> c_int {
    align_inner(memptr, alignment, size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memalign(alignment: size_t, size: size_t) -> *mut c_void {
    memalign_inner(alignment, size)
}

static MEMALIGN: unsafe extern "C" fn(alignment: size_t, size: size_t) -> *mut c_void = memalign;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aligned_alloc(alignment: size_t, size: size_t) -> *mut c_void {
    if alignment == 0 || !alignment.is_power_of_two() {
        return null_mut();
    }
    (MEMALIGN)(alignment, size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valloc(size: size_t) -> *mut c_void {
    (MEMALIGN)(4096, size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pvalloc(size: size_t) -> *mut c_void {
    let page_size = 4096;
    let rounded_size = if size == 0 {
        page_size
    } else {
        align_to(size, page_size)
    };

    (MEMALIGN)(page_size, rounded_size)
}
