use crate::{inner::calloc::calloc_inner, internals::size_t};
use std::os::raw::c_void;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(nmemb: size_t, size: size_t) -> *mut c_void {
    calloc_inner(nmemb, size)
}
