use std::{os::raw::c_void, ptr::read_volatile, sync::atomic::Ordering};

use crate::{
    MAGIC, OX_ALIGN_TAG, OxHeader, OxidallocError, TOTAL_IN_USE, TOTAL_OPS,
    big_allocation::big_free,
    slab::{match_size_class, thread_local::ThreadLocalEngine},
    va::va_helper::is_ours,
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    unsafe {
        TOTAL_OPS.fetch_add(1, Ordering::Relaxed);

        if ptr.is_null() {
            return;
        }

        if !is_ours(ptr as usize) {
            return;
        }

        let mut header_search_ptr = ptr;
        let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
        let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;
        if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
            let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
            if is_ours(presumed_original_ptr as usize) {
                header_search_ptr = presumed_original_ptr;
            }
        }

        let header = (header_search_ptr as *mut OxHeader).sub(1);
        let magic = read_volatile(&(*header).magic);
        let in_use = read_volatile(&(*header).in_use);

        if magic != MAGIC && magic != 0 {
            OxidallocError::MemoryCorruption.log_and_abort(
                header as *mut c_void,
                "Possibly Double Free",
                None,
            );
        }

        if in_use == 0 {
            OxidallocError::DoubleFree.log_and_abort(
                header as *mut c_void,
                "Pointer is tagged as in_use",
                None,
            );
        }

        let size = (*header).size as usize;
        let thread = ThreadLocalEngine::get_or_init();

        let class = match match_size_class(size) {
            Some(class) => class,
            None => {
                big_free(ptr);
                return;
            }
        };

        (*header).in_use = 0;
        (*header).magic = 0;

        TOTAL_IN_USE.fetch_sub(1, Ordering::Relaxed);
        thread.push_to_thread(class, header);
    }
}
