#[cfg(not(feature = "global-alloc"))]
use std::os::raw::{c_char, c_int, c_void};

pub mod env;
pub mod hashmap;
pub mod lock;
pub mod once;
pub mod oncelock;

unsafe extern "C" {
    #[cfg(not(feature = "global-alloc"))]
    pub fn __errno_location() -> *mut c_int;
    #[cfg(not(feature = "global-alloc"))]
    pub fn pthread_atfork(
        prepare: Option<unsafe extern "C" fn()>,
        parent: Option<unsafe extern "C" fn()>,
        child: Option<unsafe extern "C" fn()>,
    ) -> c_int;
    #[cfg(not(feature = "global-alloc"))]
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[allow(non_camel_case_types)]
pub type size_t = usize;
#[cfg(not(feature = "global-alloc"))]
pub const RTLD_NEXT: *mut c_void = 0xFFFFFFFFFFFFFFFF as *mut c_void;
