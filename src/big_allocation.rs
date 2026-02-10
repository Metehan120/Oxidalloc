use crate::{
    FREED_MAGIC, HEADER_SIZE, MAGIC, OX_DISABLE_THP, OX_FORCE_THP, OxHeader, OxidallocError,
    internals::hashmap::{BIG_ALLOC_MAP, BigAllocMeta},
    sys::memory_system::{MadviseFlags, RMProtFlags, madvise, protect_memory},
    va::{align_to, bitmap::VA_MAP},
};
use std::{
    os::raw::c_void,
    ptr::{null_mut, write},
};

use crate::internals::lock::SerialLock;

#[derive(Copy, Clone)]
struct BigFreeBlock {
    ptr: *mut OxHeader,
    total_size: usize,
}

struct BigFreeCache {
    lock: SerialLock,
    data: [Option<BigFreeBlock>; 16],
}

impl BigFreeCache {
    const fn new() -> Self {
        Self {
            lock: SerialLock::new(),
            data: [None; 16],
        }
    }
}

static mut BIG_FREE_CACHE: BigFreeCache = BigFreeCache::new();

pub unsafe fn big_malloc(size: usize) -> *mut u8 {
    let aligned_total = if OX_FORCE_THP {
        align_to(size + HEADER_SIZE, 1024 * 1024 * 2)
    } else {
        align_to(size + HEADER_SIZE, 4096)
    };

    let mut actual_ptr = null_mut();
    {
        let _guard = BIG_FREE_CACHE.lock.lock();
        let cache = &mut BIG_FREE_CACHE.data;
        for i in 0..cache.len() {
            if let Some(block) = cache[i] {
                if block.total_size >= aligned_total {
                    actual_ptr = block.ptr;
                    let block_total = block.total_size;
                    cache[i] = None;

                    let min_split = if OX_FORCE_THP { 1024 * 1024 * 2 } else { 4096 };
                    if block_total >= aligned_total + min_split {
                        let remainder_ptr = (actual_ptr as usize + aligned_total) as *mut OxHeader;
                        let remainder_size = block_total - aligned_total;

                        let mut pushed = false;
                        for j in 0..cache.len() {
                            if cache[j].is_none() {
                                cache[j] = Some(BigFreeBlock {
                                    ptr: remainder_ptr,
                                    total_size: remainder_size,
                                });
                                pushed = true;
                                break;
                            }
                        }

                        if !pushed {
                            VA_MAP.free(remainder_ptr as usize, remainder_size);
                        }
                    }
                    break;
                }
            }
        }
    }

    if actual_ptr.is_null() {
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

        if is_err {
            VA_MAP.free(hint, aligned_total);
            return null_mut();
        }
        actual_ptr = hint as *mut OxHeader;
    }

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

    let total_size = if OX_FORCE_THP {
        align_to(payload_size + HEADER_SIZE, 1024 * 1024 * 2)
    } else {
        align_to(payload_size + HEADER_SIZE, 4096)
    };

    (*header).magic = FREED_MAGIC;

    if total_size % (1024 * 1024 * 2) == 0 && !OX_DISABLE_THP {
        let _ = madvise(header as *mut c_void, total_size, MadviseFlags::NORMAL);
    }

    let mut pushed = false;
    {
        let _guard = BIG_FREE_CACHE.lock.lock();
        let cache = &mut BIG_FREE_CACHE.data;
        for i in 0..cache.len() {
            if cache[i].is_none() {
                cache[i] = Some(BigFreeBlock {
                    ptr: header,
                    total_size,
                });
                pushed = true;
                break;
            }
        }

        if !pushed {
            if let Some(to_trim) = cache[0] {
                let _ = madvise(
                    to_trim.ptr as *mut c_void,
                    to_trim.total_size,
                    MadviseFlags::DONTNEED,
                );
                VA_MAP.free(to_trim.ptr as usize, to_trim.total_size);
            }
            for i in 0..cache.len() - 1 {
                cache[i] = cache[i + 1];
            }
            cache[cache.len() - 1] = Some(BigFreeBlock {
                ptr: header,
                total_size,
            });
        }
    }
}

pub unsafe fn trim_big_allocations() -> usize {
    let _guard = BIG_FREE_CACHE.lock.lock();
    let cache = &mut BIG_FREE_CACHE.data;
    let mut freed = 0;
    for i in 0..cache.len() {
        if let Some(block) = cache[i] {
            let _ = madvise(
                block.ptr as *mut c_void,
                block.total_size,
                MadviseFlags::DONTNEED,
            );
            VA_MAP.free(block.ptr as usize, block.total_size);
            freed += block.total_size;
            cache[i] = None;
        }
    }
    freed
}
