use std::{
    os::raw::c_void,
    ptr::{null_mut, write},
    sync::atomic::Ordering,
};

use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    Err, FREED_MAGIC, HEADER_SIZE, MetaData, OX_CURRENT_STAMP, OX_USE_THP, OxHeader,
    OxidallocError, TOTAL_ALLOCATED,
    slab::{ITERATIONS, NUM_SIZE_CLASSES, SIZE_CLASSES, thread_local::ThreadLocalEngine},
    va::{align_to, bitmap::VA_MAP},
};

const LAZY_INIT_MAX: usize = 16;

#[inline(always)]
unsafe fn remaining_blocks(metadata: *mut MetaData, block_size: usize) -> usize {
    let remaining_bytes = (*metadata).end.saturating_sub((*metadata).next);
    remaining_bytes / block_size
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn init_blocks(
    metadata: *mut MetaData,
    block_size: usize,
    payload_size: usize,
    max_blocks: usize,
    current_stamp: usize,
) -> (*mut OxHeader, *mut OxHeader, usize) {
    let remaining = remaining_blocks(metadata, block_size);
    if remaining == 0 {
        return (null_mut(), null_mut(), 0);
    }

    let count = remaining.min(max_blocks);
    let base = (*metadata).next;
    let mut head = null_mut();
    let mut tail = null_mut();

    for i in (0..count).rev() {
        let current_header = (base + i * block_size) as *mut OxHeader;

        write(
            current_header,
            OxHeader {
                next: head,
                size: payload_size,
                magic: FREED_MAGIC,
                life_time: current_stamp,
                in_use: 0,
                metadata: metadata,
            },
        );

        if head.is_null() {
            tail = current_header;
        }
        head = current_header;
    }

    (*metadata).next = base + (count * block_size);

    (head, tail, count)
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn bulk_fill(thread: &ThreadLocalEngine, class: usize) -> Result<(), Err> {
    let payload_size = SIZE_CLASSES[class];
    let block_size = align_to(payload_size + HEADER_SIZE, 16);
    let num_blocks = ITERATIONS[class];
    let max_init = num_blocks.min(LAZY_INIT_MAX).max(1);
    let current_stamp = OX_CURRENT_STAMP.load(Ordering::Relaxed);

    let pending = thread.pending[class].load(Ordering::Relaxed);
    if !pending.is_null() {
        let (head, tail, count) =
            init_blocks(pending, block_size, payload_size, max_init, current_stamp);
        if count > 0 {
            thread.push_to_thread_tailed(class, head, tail, count);
            if remaining_blocks(pending, block_size) == 0 {
                thread.pending[class].store(null_mut(), Ordering::Relaxed);
            }
            return Ok(());
        }
        thread.pending[class].store(null_mut(), Ordering::Relaxed);
    }

    let total = size_of::<MetaData>() + (block_size * num_blocks);

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

    let metadata = mem as *mut MetaData;
    write(
        metadata,
        MetaData {
            start: mem as usize,
            end: (mem as usize) + total,
            next: (mem as usize) + size_of::<MetaData>(),
        },
    );

    if class == NUM_SIZE_CLASSES && OX_USE_THP.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = madvise(mem, total, Advice::LinuxHugepage);
    }

    let (head, tail, count) =
        init_blocks(metadata, block_size, payload_size, max_init, current_stamp);
    if count == 0 {
        return Err(Err::OutOfMemory);
    }

    thread.push_to_thread_tailed(class, head, tail, count);
    TOTAL_ALLOCATED.fetch_add(num_blocks, Ordering::Relaxed);

    if remaining_blocks(metadata, block_size) > 0 {
        thread.pending[class].store(metadata, Ordering::Relaxed);
    }

    Ok(())
}
