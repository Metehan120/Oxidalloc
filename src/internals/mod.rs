use std::os::raw::c_int;

pub mod env;
pub mod hashmap;
pub mod lock;
pub mod once;
pub mod oncelock;

unsafe extern "C" {
    pub fn __errno_location() -> *mut c_int;
}

#[allow(non_camel_case_types)]
pub type size_t = usize;
