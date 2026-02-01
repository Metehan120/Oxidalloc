use crate::{
    FREED_MAGIC, HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OX_CURRENT_STAMP, OxHeader, OxidallocError,
    abi::{
        fallback::free_fallback,
        malloc::{HOT_READY, TOTAL_MALLOC_FREE},
    },
    big_allocation::big_free,
    internals::size_t,
    slab::{TLS_MAX_BLOCKS, global::GlobalHandler, thread_local::ThreadLocalEngine},
    va::is_ours,
};
use std::{
    hint::{likely, unlikely},
    os::raw::c_void,
    ptr::{null_mut, read_volatile},
    sync::atomic::Ordering,
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

macro_rules! free_main {
    ($ptr:expr) => {{
        if unlikely($ptr.is_null()) {
            return;
        }

        if unlikely(!is_ours($ptr as usize)) {
            free_fallback($ptr);
            return;
        }

        let mut header_search_ptr = $ptr;
        let tag_loc = ($ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;

        if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
            let raw_loc = ($ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;
            let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
            if is_ours(presumed_original_ptr as usize) {
                header_search_ptr = presumed_original_ptr;
            }
        }

        free_internal(header_search_ptr);
    }};
}

#[inline(always)]
pub unsafe fn validate_ptr_for_abi(header: *mut OxHeader) {
    let magic = read_volatile(&(*header).magic);
    if likely(magic == MAGIC) {
        return;
    }

    if magic == FREED_MAGIC {
        OxidallocError::DoubleFree.log_and_abort(
            header as *mut c_void,
            "Pointer is tagged as in_use",
            None,
        );
    }

    OxidallocError::AttackOrCorruption.log_and_abort(
        null_mut() as *mut c_void,
        "Attack or corruption detected; aborting process. External system access and RAM module checks recommended.",
        None,
    );
}

#[inline(always)]
unsafe fn free_internal(ptr: *mut c_void) {
    let header_addr = (ptr as usize).wrapping_sub(HEADER_SIZE);
    let header = header_addr as *mut OxHeader;

    validate_ptr_for_abi(header);

    let class = (*header).class as usize;
    if unlikely(class == 100) {
        big_free(ptr as *mut OxHeader);
        return;
    }

    (*header).magic = FREED_MAGIC;
    (*header).life_time = OX_CURRENT_STAMP;

    let thread = ThreadLocalEngine::get_or_init();
    if thread.tls[class].usage >= TLS_MAX_BLOCKS[class] {
        GlobalHandler.push_to_global(class, header, header, 1);
        return;
    };

    thread.push_to_thread(class, header);
}

#[inline(always)]
unsafe fn free_fast(ptr: *mut c_void) {
    free_main!(ptr)
}

#[cold]
#[inline(never)]
unsafe fn free_boot_segment(ptr: *mut c_void) {
    TOTAL_MALLOC_FREE.fetch_add(1, Ordering::Relaxed);

    free_main!(ptr)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    if likely(HOT_READY) {
        free_fast(ptr);
    } else {
        free_boot_segment(ptr);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_sized(ptr: *mut c_void, _: size_t) {
    free(ptr);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_aligned_sized(ptr: *mut c_void, _: size_t, _: size_t) {
    free(ptr);
}
