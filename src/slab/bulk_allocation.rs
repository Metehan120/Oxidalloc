use std::{
    os::raw::c_void,
    ptr::{null_mut, write},
    sync::atomic::Ordering,
};

use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    Err, HEADER_SIZE, MAGIC, OX_CURRENT_STAMP, OX_USE_THP, OxHeader, OxidallocError,
    TOTAL_ALLOCATED,
    slab::{ITERATIONS, NUM_SIZE_CLASSES, SIZE_CLASSES, thread_local::ThreadLocalEngine},
    va::{align_to, bitmap::VA_MAP},
};

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn bulk_fill(thread: &ThreadLocalEngine, class: usize) -> Result<(), Err> {
    let payload_size = SIZE_CLASSES[class];
    let block_size = align_to(payload_size + HEADER_SIZE, 16);
    let num_blocks = ITERATIONS[class];
    let total = block_size * num_blocks;

    let hint = VA_MAP.alloc(total).unwrap_or_else(|| {
        OxidallocError::VaBitmapExhausted.log_and_abort(
            null_mut(),
            "VA bitmap exhausted | This is expected",
            None,
        )
    });

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

    if class == NUM_SIZE_CLASSES && OX_USE_THP.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = madvise(mem, total, Advice::LinuxHugepage);
    }

    let current_stamp = OX_CURRENT_STAMP.load(Ordering::Relaxed);
    let mut prev = null_mut();
    for i in (0..num_blocks).rev() {
        let offset = i * block_size;
        let current_header = (mem as usize + offset) as *mut OxHeader;

        write(
            current_header,
            OxHeader {
                next: prev,
                size: payload_size,
                magic: MAGIC,
                flag: 0,
                life_time: current_stamp,
                in_use: 0,
                used_before: 0,
            },
        );

        prev = current_header;
    }

    let mut tail = prev;
    for _ in 0..num_blocks - 1 {
        tail = (*tail).next;
    }

    thread.push_to_thread_tailed(class, prev, tail, num_blocks);
    TOTAL_ALLOCATED.fetch_add(num_blocks, Ordering::Relaxed);

    Ok(())
}
