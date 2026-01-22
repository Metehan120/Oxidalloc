use rustix::mm::{Advice, MapFlags, MprotectFlags, ProtFlags, madvise, mmap_anonymous, mprotect};

use crate::{
    FREED_MAGIC, HEADER_SIZE, MAGIC, OX_USE_THP, OxHeader,
    va::{align_to, bitmap::VA_MAP},
};
use std::{
    os::raw::c_void,
    ptr::{null_mut, write, write_bytes},
};

pub unsafe fn big_malloc(size: usize) -> *mut u8 {
    // Align size to the page size so we don't explode later
    let aligned_total = align_to(size + HEADER_SIZE, 4096);

    // Reserve virtual space first
    let hint = match VA_MAP.alloc(aligned_total) {
        Some(hint) => hint,
        None => return null_mut(),
    };

    let is_err = mprotect(
        hint as *mut c_void,
        aligned_total,
        MprotectFlags::WRITE | MprotectFlags::READ,
    )
    .is_err();

    let actual_ptr = if is_err {
        match mmap_anonymous(
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
        }
    } else {
        hint as *mut c_void
    } as *mut OxHeader;

    if OX_USE_THP.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = madvise(
            actual_ptr as *mut c_void,
            aligned_total,
            Advice::LinuxHugepage,
        );
    }

    write(
        actual_ptr,
        OxHeader {
            next: null_mut(),
            size,
            class: 100,
            magic: MAGIC,
            life_time: 0,
            metadata: null_mut(),
        },
    );

    (actual_ptr as *mut u8).add(HEADER_SIZE)
}

pub unsafe fn big_free(ptr: *mut OxHeader) {
    let header = ptr.sub(1);
    let payload_size = (*header).size as usize;

    // Align size back to original size
    let total_size = align_to(payload_size + HEADER_SIZE, 4096);

    // Make the header look free before we potentially lose write access.
    (*header).magic = FREED_MAGIC;

    let is_failed = madvise(header as *mut c_void, total_size, Advice::LinuxDontNeed);
    if is_failed.is_err() {
        // Security: Zero out the memory before freeing it so it wont leak the info
        write_bytes(header as *mut u8, 0, total_size);
    }

    let _ = mprotect(header as *mut c_void, total_size, MprotectFlags::empty());

    VA_MAP.free(header as usize, total_size);
}
