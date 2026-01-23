use crate::{HEADER_SIZE, OxHeader, abi::malloc::malloc};
use libc::{__errno_location, ENOMEM, size_t};
use std::{alloc::Layout, os::raw::c_void, ptr::null_mut};

#[inline(always)]
unsafe fn calc_and_get(size: Layout, nmem: usize) -> Option<(*mut c_void, usize)> {
    let size = size.size();
    let total_size = match nmem.checked_mul(size) {
        Some(s) => s,
        None => {
            *__errno_location() = ENOMEM;
            return None;
        }
    };

    let effective_size = if total_size == 0 { 1 } else { total_size };
    let ptr = malloc(effective_size);
    if ptr.is_null() {
        return None;
    }
    Some((ptr, effective_size))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(nmemb: size_t, size: size_t) -> *mut c_void {
    let layout = match Layout::array::<u8>(size) {
        Ok(layout) => layout,
        Err(_) => {
            *__errno_location() = ENOMEM;
            return null_mut();
        }
    };
    let (ptr, effective_size) = match calc_and_get(layout, nmemb) {
        Some(ptr) => ptr,
        None => return null_mut(),
    };

    if !ptr.is_null() {
        unsafe {
            let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

            let actual_size = (*header).size;
            std::ptr::write_bytes(ptr as *mut u8, 0, actual_size.min(effective_size));
        }
    }

    ptr
}
