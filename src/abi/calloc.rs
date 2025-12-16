use crate::{HEADER_SIZE, OxHeader, TOTAL_OPS, abi::malloc::malloc};
use libc::{__errno_location, ENOMEM, size_t};
use std::{os::raw::c_void, ptr::null_mut};

#[unsafe(no_mangle)]
pub extern "C" fn calloc(nmemb: size_t, size: size_t) -> *mut c_void {
    TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let total_size = match nmemb.checked_mul(size) {
        Some(s) => s,
        None => {
            unsafe { *__errno_location() = ENOMEM };
            return null_mut();
        }
    };

    // glibc returns a non-null pointer for zero-sized calloc; keep parity by
    // allocating the smallest block we can hand back.
    let effective_size = if total_size == 0 { 1 } else { total_size };

    let ptr = malloc(effective_size);
    if ptr.is_null() {
        unsafe { *__errno_location() = ENOMEM };
        return null_mut();
    }

    if !ptr.is_null() {
        unsafe {
            let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

            let actual_size = (*header).size as usize;
            std::ptr::write_bytes(ptr as *mut u8, 0, actual_size.min(effective_size));
        }
    }

    ptr
}
