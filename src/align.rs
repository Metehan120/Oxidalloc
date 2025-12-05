use crate::malloc::malloc;
use libc::size_t;
use std::{
    os::raw::{c_int, c_void},
    ptr::null_mut,
};

const OFFSET_SIZE: usize = size_of::<usize>();

#[unsafe(no_mangle)]
pub extern "C" fn posix_memalign(memptr: *mut *mut c_void, alignment: usize, size: usize) -> c_int {
    if memptr.is_null() {
        return libc::EINVAL;
    }

    unsafe {
        let (total1, overflow1) = size.overflowing_add(alignment);

        let (total_requested, overflow2) = total1.overflowing_add(OFFSET_SIZE);

        if overflow1 || overflow2 {
            return libc::ENOMEM;
        }

        let raw = malloc(total_requested);
        if raw.is_null() {
            return libc::ENOMEM;
        }

        let addr = raw as usize;

        let start_search = addr.saturating_add(OFFSET_SIZE);

        let aligned = (start_search + alignment - 1) & !(alignment - 1);

        let original_ptr_location = aligned.saturating_sub(OFFSET_SIZE) as *mut usize;
        *original_ptr_location = (raw as usize) | 1;

        *memptr = aligned as *mut c_void;

        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn memalign(alignment: size_t, size: size_t) -> *mut c_void {
    let mut ptr: *mut c_void = null_mut();
    if posix_memalign(&mut ptr, alignment, size) == 0 {
        ptr
    } else {
        null_mut()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aligned_alloc(alignment: size_t, size: size_t) -> *mut c_void {
    memalign(alignment, size)
}

#[unsafe(no_mangle)]
pub extern "C" fn valloc(size: size_t) -> *mut c_void {
    memalign(4096, size)
}
