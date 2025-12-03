use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use crate::{
    OxHeader, TOTAL_OPS,
    free::{free, is_ours},
    internals::{AllocationHelper, MAGIC, VA_END, VA_START},
    malloc::malloc,
};

// TODO: mremap logic
#[unsafe(no_mangle)]
pub extern "C" fn realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    if ptr.is_null() {
        return malloc(new_size);
    }

    if !is_ours(ptr) {
        return null_mut();
    }

    TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if new_size > VA_END.load(Ordering::Relaxed) - VA_START.load(Ordering::Relaxed) {
        return null_mut();
    }

    if new_size == 0 {
        free(ptr);
        return malloc(1);
    }

    unsafe {
        let header = (ptr as *mut OxHeader).sub(1);

        if (*header).magic != MAGIC && (*header).magic != 0 {
            return null_mut();
        }

        let old_size = (*header).size as usize;

        let old_class = AllocationHelper.match_size_class(old_size);
        let new_class = AllocationHelper.match_size_class(new_size);

        if new_class.is_some() && new_class == old_class {
            return ptr;
        }

        let new_ptr = malloc(new_size);
        if new_ptr.is_null() {
            return std::ptr::null_mut();
        }

        std::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, old_size.min(new_size));

        free(ptr);
        new_ptr
    }
}
