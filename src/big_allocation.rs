#![allow(unsafe_op_in_unsafe_fn)]

use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    HEADER_SIZE, MAGIC, OX_USE_THP, OxHeader,
    va::{align_to, bitmap::VA_MAP},
};
use std::{
    os::raw::c_void,
    ptr::{null_mut, write_bytes},
};

pub unsafe fn big_malloc(size: usize) -> *mut u8 {
    // Align size to the page size so we don't explode later
    let aligned_total = align_to(size + HEADER_SIZE, 4096);

    // Reserve virtual space first
    let hint = match VA_MAP.alloc(aligned_total) {
        Some(hint) => hint,
        None => return null_mut(),
    };

    let actual_ptr = match mmap_anonymous(
        hint as *mut c_void,
        aligned_total,
        ProtFlags::WRITE | ProtFlags::READ,
        MapFlags::PRIVATE | MapFlags::FIXED,
    ) {
        Ok(ptr) => ptr,
        Err(_) => {
            VA_MAP.free(hint, aligned_total);
            return null_mut();
        }
    } as *mut OxHeader;

    if OX_USE_THP.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = madvise(
            actual_ptr as *mut c_void,
            aligned_total,
            Advice::LinuxHugepage,
        );
    }

    // Initialize the header
    (*actual_ptr).size = size;
    (*actual_ptr).magic = MAGIC;
    (*actual_ptr).in_use = 1;

    (actual_ptr as *mut u8).add(HEADER_SIZE)
}

pub unsafe fn big_free(ptr: *mut OxHeader) {
    let header = ptr.sub(1);
    let payload_size = (*header).size as usize;

    // Align size back to original size
    let total_size = align_to(payload_size + HEADER_SIZE, 4096);

    // Make the header look free before we potentially lose write access.
    (*header).in_use = 0;
    (*header).magic = 0;

    // If this fails (e.g. under a restrictive sandbox), fall back to `madvise(DONTNEED)`.
    let remap_result = mmap_anonymous(
        header as *mut c_void,
        total_size,
        ProtFlags::empty(),
        MapFlags::PRIVATE | MapFlags::FIXED | MapFlags::NORESERVE,
    );

    if remap_result.is_err() {
        let is_failed = madvise(header as *mut c_void, total_size, Advice::LinuxDontNeed);
        if is_failed.is_err() {
            write_bytes(header as *mut u8, 0, total_size);
        }
    }

    VA_MAP.free(header as usize, total_size);
}
