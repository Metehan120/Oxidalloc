use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    Err, HEADER_SIZE, MAGIC, OxHeader, TOTAL_ALLOCATED,
    slab::{ITERATIONS, SIZE_CLASSES, thread_local::ThreadLocalEngine, unpack_header},
    va::{align_to, bitmap::VA_MAP, bootstrap::GLOBAL_RANDOM},
};

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn bulk_fill(thread: &ThreadLocalEngine, class: usize) -> Result<(), Err> {
    let payload_size = SIZE_CLASSES[class];
    let block_size = align_to(payload_size + HEADER_SIZE, 16);
    let num_blocks = ITERATIONS[class];
    let total = block_size * num_blocks;

    let hint = VA_MAP.alloc(total).ok_or(Err::OutOfReservation)?;
    let mem = mmap_anonymous(
        hint as *mut c_void,
        total,
        ProtFlags::WRITE | ProtFlags::READ,
        MapFlags::PRIVATE | MapFlags::FIXED,
    )
    .map_err(|_| {
        VA_MAP.free(hint, total);
        Err::OutOfMemory
    })?;

    // For 2MB size class use THP which uses 2MB pages
    if class == 17 {
        match madvise(mem, total, Advice::LinuxHugepage) {
            Ok(_) => (),
            Err(_) => (),
        };
    }

    let mut prev = null_mut();
    for i in (0..num_blocks).rev() {
        let offset = i * block_size;
        let current_header = (mem as usize + offset) as *mut OxHeader;

        (*current_header).next = (prev as usize) ^ GLOBAL_RANDOM;
        (*current_header).size = payload_size as u64;
        (*current_header).magic = MAGIC;
        (*current_header).in_use = 0;

        prev = current_header;
    }

    let mut tail = prev;
    for _ in 0..num_blocks - 1 {
        tail = unpack_header((*tail).next, GLOBAL_RANDOM);
    }

    thread.push_to_thread_tailed(class, prev, tail, num_blocks);
    TOTAL_ALLOCATED.fetch_add(num_blocks, Ordering::Relaxed);

    Ok(())
}
