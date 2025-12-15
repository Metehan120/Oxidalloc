use libc::__errno_location;
use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    HEADER_SIZE, MAGIC, OxHeader,
    va::{align_to, bitmap::VA_MAP},
};
use std::{os::raw::c_void, ptr::null_mut};

pub fn big_malloc(size: usize) -> *mut c_void {
    unsafe {
        let aligned_total = align_to(size, 4096);
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
            Err(_) => return null_mut(),
        } as *mut OxHeader;

        (*actual_ptr).size = aligned_total as u64;
        (*actual_ptr).magic = MAGIC;
        (*actual_ptr).in_use = 1;

        (actual_ptr as *mut u8).add(HEADER_SIZE) as *mut c_void
    }
}

pub fn big_free(ptr: *mut c_void) {
    unsafe {
        let header = (ptr as *mut OxHeader).sub(1);
        let size = (*header).size as usize;
        match madvise(header as *mut c_void, size, Advice::LinuxDontNeed) {
            Ok(_) => VA_MAP.free(ptr as usize, size),
            Err(_) => eprintln!(
                "Madvise Failed, memory leaked. size={}, errno={}",
                size,
                *__errno_location()
            ),
        };
    }
}
