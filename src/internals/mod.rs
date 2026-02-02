use std::os::raw::{c_char, c_int, c_void};

pub mod env;
pub mod hashmap;
pub mod lock;
pub mod once;
pub mod oncelock;

unsafe extern "C" {
    pub fn __errno_location() -> *mut c_int;
    pub fn pthread_atfork(
        prepare: Option<unsafe extern "C" fn()>,
        parent: Option<unsafe extern "C" fn()>,
        child: Option<unsafe extern "C" fn()>,
    ) -> c_int;
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[allow(non_camel_case_types)]
pub type size_t = usize;
pub const RTLD_NEXT: *mut c_void = 0xFFFFFFFFFFFFFFFF as *mut c_void;
