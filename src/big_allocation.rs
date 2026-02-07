use crate::{
    FREED_MAGIC, HEADER_SIZE, MAGIC, OX_DISABLE_THP, OX_FORCE_THP, OxHeader, OxidallocError,
    internals::hashmap::{BIG_ALLOC_MAP, BigAllocMeta},
    sys::memory_system::{
        MMapFlags, MProtFlags, MadviseFlags, MemoryFlags, RMProtFlags, madvise, mmap_memory,
        protect_memory,
    },
    va::{align_to, bitmap::VA_MAP},
};
use std::{
    os::raw::c_void,
    ptr::{null_mut, write, write_bytes},
};

pub unsafe fn big_malloc(size: usize) -> *mut u8 {
    // Align size to the page size so we don't explode later
    let aligned_total = if OX_FORCE_THP {
        align_to(size + HEADER_SIZE, 1024 * 1024 * 2)
    } else {
        align_to(size + HEADER_SIZE, 4096)
    };

    // Reserve virtual space first
    let hint = match VA_MAP.alloc(aligned_total) {
        Some(hint) => hint,
        None => return null_mut(),
    };

    let is_err = protect_memory(
        hint as *mut c_void,
        aligned_total,
        RMProtFlags::WRITE | RMProtFlags::READ,
    )
    .is_err();

    let actual_ptr = if is_err {
        match mmap_memory(
            hint as *mut c_void,
            aligned_total,
            MMapFlags {
                prot: MProtFlags::WRITE | MProtFlags::READ,
                map: MemoryFlags::PRIVATE | MemoryFlags::FIXED,
            },
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

    if aligned_total % (1024 * 1024 * 2) == 0 && !OX_DISABLE_THP {
        let _ = madvise(
            actual_ptr as *mut c_void,
            aligned_total,
            MadviseFlags::HUGEPAGE,
        );
    }

    write(
        actual_ptr,
        OxHeader {
            next: null_mut(),
            class: 100,
            magic: MAGIC,
            life_time: 0,
        },
    );

    BIG_ALLOC_MAP.insert(
        actual_ptr as usize,
        BigAllocMeta {
            size,
            class: 100,
            life_time: 0,
            flags: 0,
        },
    );

    (actual_ptr as *mut u8).add(HEADER_SIZE)
}

pub unsafe fn big_free(ptr: *mut OxHeader) {
    let header = ptr.sub(1);
    let payload_size = BIG_ALLOC_MAP
        .remove(header as usize)
        .map(|meta| meta.size)
        .unwrap_or_else(|| {
            OxidallocError::AttackOrCorruption.log_and_abort(
                header as *mut c_void,
                "Missing big allocation metadata during free",
                None,
            )
        });

    // Align size back to original size
    let total_size = if OX_FORCE_THP {
        align_to(payload_size + HEADER_SIZE, 1024 * 1024 * 2)
    } else {
        align_to(payload_size + HEADER_SIZE, 4096)
    };

    // Make the header look free before we potentially lose write access.
    (*header).magic = FREED_MAGIC;

    let is_failed = madvise(header as *mut c_void, total_size, MadviseFlags::DONTNEED);
    if is_failed.is_err() {
        // Security: Zero out the memory before freeing it so it wont leak the info
        write_bytes(header as *mut u8, 0, total_size);
    }

    if total_size % (1024 * 1024 * 2) == 0 && !OX_DISABLE_THP {
        let _ = madvise(header as *mut c_void, total_size, MadviseFlags::NORMAL);
    }
    let _ = protect_memory(header as *mut c_void, total_size, RMProtFlags::NONE);

    VA_MAP.free(header as usize, total_size);
}
