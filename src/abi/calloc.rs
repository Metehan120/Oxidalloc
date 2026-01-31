use crate::{
    HEADER_SIZE, OxHeader, OxidallocError,
    abi::malloc::malloc,
    internals::{__errno_location, hashmap::BIG_ALLOC_MAP, size_t},
    slab::SIZE_CLASSES,
    sys::NOMEM,
};
use std::{alloc::Layout, os::raw::c_void, ptr::null_mut};

#[inline(always)]
unsafe fn calc_and_get(size: Layout, nmem: usize) -> Option<(*mut c_void, usize)> {
    let size = size.size();
    let total_size = match nmem.checked_mul(size) {
        Some(s) => s,
        None => {
            *__errno_location() = NOMEM;
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
            *__errno_location() = NOMEM;
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

            let mut actual_size = (*header).class as usize;
            if actual_size == 100 {
                let payload_size = BIG_ALLOC_MAP
                    .get(header as usize)
                    .map(|meta| meta.size)
                    .unwrap_or_else(|| {
                        OxidallocError::AttackOrCorruption.log_and_abort(
                            header as *mut c_void,
                            "Missing big allocation metadata during calloc",
                            None,
                        )
                    });

                actual_size = payload_size;
            } else {
                actual_size = SIZE_CLASSES[actual_size]
            }

            std::ptr::write_bytes(ptr as *mut u8, 0, actual_size.min(effective_size) as usize);
        }
    }

    ptr
}
