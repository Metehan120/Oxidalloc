use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{madvise, mmap_anonymous, Advice, MapFlags, ProtFlags};

use crate::{
    slab::{thread_local::ThreadLocalEngine, ITERATIONS, SIZE_CLASSES},
    va::{align_to, bitmap::VA_MAP},
    Err, OxHeader, HEADER_SIZE, MAGIC, TOTAL_ALLOCATED,
};

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn bulk_fill(thread: &ThreadLocalEngine, class: usize) -> Result<(), Err> {
    let payload_size = SIZE_CLASSES[class];
    let block_size = align_to(payload_size + HEADER_SIZE, 16);
    let total = align_to(block_size * ITERATIONS[class], 4096);

    // First reserve virtual space
    let hint = match VA_MAP.alloc(total) {
        Some(size) => size,
        None => return Err(Err::OutOfReservation),
    };

    let mem = match mmap_anonymous(
        hint as *mut c_void,
        total,
        ProtFlags::WRITE | ProtFlags::READ,
        MapFlags::PRIVATE | MapFlags::FIXED,
    ) {
        Ok(mem) => mem,
        Err(_) => {
            VA_MAP.free(hint, total);
            return Err(Err::OutOfMemory);
        }
    };

    // For 2MB size class use THP which uses 2MB pages
    if class == 19 {
        match madvise(mem, total, Advice::LinuxHugepage) {
            Ok(_) => (),
            Err(_) => (),
        };
    }

    let total = total / block_size;
    let mut prev = null_mut();
    for i in (0..total).rev() {
        let current_header = (mem as usize + i * block_size) as *mut OxHeader;
        (*current_header).next = prev;
        (*current_header).size = payload_size as u64;
        (*current_header).magic = MAGIC;
        (*current_header).in_use = 0;
        prev = current_header;
    }

    let mut tail = prev;
    for _ in 0..total - 1 {
        tail = (*tail).next;
    }

    thread.push_to_thread_tailed(class, prev, tail, total);
    TOTAL_ALLOCATED.fetch_add(total, Ordering::Relaxed);

    Ok(())
}
