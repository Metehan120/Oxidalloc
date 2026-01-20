#![allow(unsafe_op_in_unsafe_fn)]

use libc::size_t;

use crate::{
    FREED_MAGIC, HAS_ALIGNED_PAGES, HEADER_SIZE, OX_ALIGN_TAG, OX_CURRENT_STAMP, OxHeader,
    OxidallocError,
    abi::{
        fallback::free_fallback,
        malloc::{HOT_READY, TOTAL_MALLOC_FREE, validate_ptr},
    },
    big_allocation::big_free,
    slab::{TLS_MAX_BLOCKS, global::GlobalHandler, thread_local::ThreadLocalEngine},
    va::is_ours,
};
use std::{
    hint::{likely, unlikely},
    os::raw::c_void,
    sync::atomic::Ordering,
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

#[inline(always)]
unsafe fn free_internal(ptr: *mut c_void) {
    let header_addr = (ptr as usize).wrapping_sub(HEADER_SIZE);
    let header = header_addr as *mut OxHeader;

    validate_ptr(header);

    if unlikely((*header).magic == FREED_MAGIC) {
        OxidallocError::DoubleFree.log_and_abort(
            header as *mut c_void,
            "Pointer is tagged as in_use",
            None,
        );
    }

    let class = (*header).class as usize;
    if unlikely(class == 100) {
        big_free(ptr as *mut OxHeader);
        return;
    }

    let stamp = OX_CURRENT_STAMP;

    (*header).magic = FREED_MAGIC;
    (*header).life_time = stamp;

    let thread = ThreadLocalEngine::get_or_init();
    if thread.tls[class].usage >= TLS_MAX_BLOCKS[class] {
        GlobalHandler.push_to_global(class, thread.numa_node_id, header, header, 1);
        return;
    };

    thread.push_to_thread(class, header);
}

#[inline(always)]
unsafe fn free_fast(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    if !is_ours(ptr as usize) {
        free_fallback(ptr);
        return;
    }

    let mut header_search_ptr = ptr;
    if HAS_ALIGNED_PAGES.load(Ordering::Relaxed) {
        let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;

        if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
            let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;
            let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
            if is_ours(presumed_original_ptr as usize) {
                header_search_ptr = presumed_original_ptr;
            }
        }
    }

    free_internal(header_search_ptr);
}

#[inline(always)]
unsafe fn free_boot_segment(ptr: *mut c_void) {
    TOTAL_MALLOC_FREE.fetch_add(1, Ordering::Relaxed);

    if ptr.is_null() {
        return;
    }

    if !is_ours(ptr as usize) {
        free_fallback(ptr);
        return;
    }

    let mut header_search_ptr = ptr;
    let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;

    if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
        let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;
        let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
        if is_ours(presumed_original_ptr as usize) {
            header_search_ptr = presumed_original_ptr;
        }
    }

    free_internal(header_search_ptr);
}

#[unsafe(no_mangle)]
// If we seperate free nothing will change much, free can stay naked for now
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

#[cfg(test)]
mod tests {
    use std::{hint::black_box, time::Instant};

    use crate::abi::malloc::malloc;

    use super::*;

    #[test]
    fn test_free_only_speed() {
        unsafe {
            const N: usize = 1_000;
            let mut ptrs = Vec::with_capacity(N);

            for _ in 0..N {
                ptrs.push(malloc(64));
            }

            let start = Instant::now();
            for p in ptrs {
                black_box(free(p));
            }
            let end = Instant::now();

            let ns = end.duration_since(start).as_nanos() as f64 / N as f64;
            println!("free only: {:.2} ns/op", ns);
        }
    }
}
