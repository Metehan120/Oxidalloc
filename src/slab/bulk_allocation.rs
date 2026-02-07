use std::{
    os::raw::c_void,
    ptr::{null_mut, write},
};

use crate::{
    Err, FREED_MAGIC, HEADER_SIZE, MetaData, OX_CURRENT_STAMP, OX_DISABLE_THP, OxHeader,
    OxidallocError,
    slab::{
        ITERATIONS, NUM_SIZE_CLASSES, SIZE_CLASSES, TLS_MAX_BLOCKS, global::GlobalHandler,
        thread_local::ThreadLocalEngine,
    },
    sys::memory_system::{MMapFlags, MProtFlags, MadviseFlags, MemoryFlags, madvise, mmap_memory},
    va::{align_to, bitmap::VA_MAP},
};

#[inline(always)]
unsafe fn remaining_blocks(metadata: *mut MetaData, block_size: usize) -> usize {
    let remaining_bytes = (*metadata).end.saturating_sub((*metadata).next);
    remaining_bytes / block_size
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn init_blocks(
    class: u8,
    metadata: *mut MetaData,
    block_size: usize,
    max_blocks: usize,
    current_stamp: u32,
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
                class,
                magic: FREED_MAGIC,
                life_time: current_stamp,
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
pub unsafe fn bulk_fill(thread: &mut ThreadLocalEngine, class: usize) -> Result<(), Err> {
    let payload_size = SIZE_CLASSES[class];
    let block_size = align_to(payload_size + HEADER_SIZE, 16);
    let num_blocks = ITERATIONS[class];
    let blocks_per_4k = 4096 / block_size;
    let max_init = if blocks_per_4k >= 48 {
        48
    } else {
        blocks_per_4k.max(1)
    };
    let current_stamp = OX_CURRENT_STAMP;

    let pending = thread.pending[class];
    if !pending.is_null() {
        let (head, tail, count) =
            init_blocks(class as u8, pending, block_size, max_init, current_stamp);
        if count > 0 {
            thread.push_to_thread_tailed(class, head, tail, count);
            if remaining_blocks(pending, block_size) == 0 {
                thread.pending[class] = null_mut();
            }
            return Ok(());
        }
        thread.pending[class] = null_mut();
    }

    let total = size_of::<MetaData>() + (block_size * num_blocks);

    let hint = VA_MAP.alloc(total).unwrap_or_else(|| {
        OxidallocError::VaBitmapExhausted.log_and_abort(
            null_mut(),
            "VA bitmap exhausted | This is expected",
            None,
        )
    });

    let mem = mmap_memory(
        hint as *mut c_void,
        total,
        MMapFlags {
            prot: MProtFlags::WRITE | MProtFlags::READ,
            map: MemoryFlags::PRIVATE | MemoryFlags::FIXED,
        },
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

    if class == NUM_SIZE_CLASSES - 1 && !OX_DISABLE_THP {
        let _ = madvise(
            (mem as *mut u8).add(HEADER_SIZE) as *mut c_void,
            payload_size,
            MadviseFlags::HUGEPAGE,
        );
    }

    let (head, tail, count) =
        init_blocks(class as u8, metadata, block_size, max_init, current_stamp);
    if count == 0 {
        return Err(Err::OutOfMemory);
    }

    if thread.tls[class].usage >= TLS_MAX_BLOCKS[class] {
        GlobalHandler.push_to_global(class, head, tail, count);
        if remaining_blocks(metadata, block_size) > 0 {
            thread.pending[class] = metadata;
        }
        return Ok(());
    }

    thread.push_to_thread_tailed(class, head, tail, count);
    if remaining_blocks(metadata, block_size) > 0 {
        thread.pending[class] = metadata;
    }

    Ok(())
}

pub unsafe fn drain_pending(thread: &mut ThreadLocalEngine, class: usize) {
    let pending = thread.pending[class];
    if pending.is_null() {
        return;
    }

    let payload_size = SIZE_CLASSES[class];
    let block_size = align_to(payload_size + HEADER_SIZE, 16);
    let current_stamp = OX_CURRENT_STAMP;
    let remaining = remaining_blocks(pending, block_size);

    if remaining > 0 {
        let (head, tail, count) =
            init_blocks(class as u8, pending, block_size, remaining, current_stamp);
        if count > 0 {
            GlobalHandler.push_to_global(class, head, tail, count);
        }
    }

    thread.pending[class] = null_mut();
}
