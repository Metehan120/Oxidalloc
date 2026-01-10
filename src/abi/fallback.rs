#![allow(unsafe_op_in_unsafe_fn)]

use libc::{RTLD_NEXT, c_char, c_void, dlsym, size_t};
use std::{
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicPtr, Ordering},
    },
};

type FreeFn = unsafe extern "C" fn(*mut c_void);
type ReallocFn = unsafe extern "C" fn(*mut c_void, size_t) -> *mut c_void;

static FREE_INIT: Once = Once::new();
static FREE_PTR: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

static REALLOC_INIT: Once = Once::new();
static REALLOC_PTR: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn get_symbol(name: &[u8], init: &Once, slot: &AtomicPtr<c_void>) -> *mut c_void {
    init.call_once(|| {
        let sym = unsafe { dlsym(RTLD_NEXT, name.as_ptr() as *const c_char) };
        slot.store(sym, Ordering::Relaxed);
    });
    slot.load(Ordering::Relaxed)
}

pub unsafe fn free_fallback(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let sym = get_symbol(b"free\0", &FREE_INIT, &FREE_PTR);
    if sym.is_null() {
        return;
    }

    let func: FreeFn = unsafe { std::mem::transmute(sym) };
    func(ptr);
}

pub unsafe fn realloc_fallback(ptr: *mut c_void, size: size_t) -> *mut c_void {
    let sym = get_symbol(b"realloc\0", &REALLOC_INIT, &REALLOC_PTR);
    if sym.is_null() {
        return null_mut();
    }

    let func: ReallocFn = unsafe { std::mem::transmute(sym) };
    func(ptr, size)
}
