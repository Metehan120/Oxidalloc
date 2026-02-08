use crate::{inner::free::free_inner, internals::size_t};
use std::os::raw::c_void;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    free_inner(ptr);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_sized(ptr: *mut c_void, _: size_t) {
    free(ptr);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_aligned_sized(ptr: *mut c_void, _: size_t, _: size_t) {
    free(ptr);
}
