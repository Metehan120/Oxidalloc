use std::{os::raw::c_void, ptr::null_mut};

use libc::size_t;

use crate::{HEADER_SIZE, MAGIC, OxHeader, TOTAL_OPS, abi::malloc::malloc};

#[unsafe(no_mangle)]
pub extern "C" fn calloc(nmemb: size_t, size: size_t) -> *mut c_void {
    TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let total_size = match nmemb.checked_mul(size) {
        Some(s) => s,
        None => return null_mut(),
    };

    if total_size == 0 {
        return null_mut();
    }

    let ptr = malloc(total_size);

    if !ptr.is_null() {
        unsafe {
            let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

            if (*header).magic != MAGIC {
                let actual_size = (*header).size as usize;

                std::ptr::write_bytes(ptr as *mut u8, 0, actual_size.min(total_size));
            } else {
                let actual_size = (*header).size as usize;
                std::ptr::write_bytes(ptr as *mut u8, 0, actual_size.min(total_size));
            }
        }
    }

    ptr
}
