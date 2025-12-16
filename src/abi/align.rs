use libc::size_t;
use std::{
    os::raw::{c_int, c_void},
    ptr::null_mut,
};

use crate::abi::malloc::malloc;

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

#[unsafe(no_mangle)]
pub extern "C" fn posix_memalign(memptr: *mut *mut c_void, alignment: usize, size: usize) -> c_int {
    if memptr.is_null() {
        return libc::EINVAL;
    }

    if alignment == 0 || alignment % size_of::<usize>() != 0 || (alignment & (alignment - 1)) != 0 {
        return libc::EINVAL;
    }

    unsafe {
        let total_requested = match size
            .checked_add(alignment)
            .and_then(|v| v.checked_add(TAG_SIZE))
        {
            Some(v) => v,
            None => return libc::ENOMEM,
        };

        let raw = malloc(total_requested);
        if raw.is_null() {
            return libc::ENOMEM;
        }

        let addr = raw as usize;

        let start_search = addr.saturating_add(TAG_SIZE);

        let aligned = (start_search + alignment - 1) & !(alignment - 1);

        let tag_location = aligned.saturating_sub(TAG_SIZE) as *mut usize;
        let original_ptr_location = aligned.saturating_sub(OFFSET_SIZE) as *mut usize;
        *tag_location = crate::OX_ALIGN_TAG;
        *original_ptr_location = raw as usize;

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
    if alignment == 0 || (alignment & (alignment - 1)) != 0 || size % alignment != 0 {
        return null_mut();
    }
    memalign(alignment, size)
}

#[unsafe(no_mangle)]
pub extern "C" fn valloc(size: size_t) -> *mut c_void {
    memalign(4096, size)
}
