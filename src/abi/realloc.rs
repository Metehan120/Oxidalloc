use std::{os::raw::c_void, ptr::null_mut};

use crate::{
    HEADER_SIZE, OX_ALIGN_TAG, OxHeader, OxidallocError,
    abi::{
        fallback::realloc_fallback,
        free::{free, validate_ptr_for_abi},
        malloc::malloc,
    },
    internals::{
        __errno_location,
        hashmap::{BIG_ALLOC_MAP, BigAllocMeta},
        size_t,
    },
    slab::{ITERATIONS, SIZE_CLASSES, match_size_class},
    sys::{
        NOMEM,
        memory_system::{MadviseFlags, RMProtFlags, madvise, protect_memory},
    },
    va::{align_to, bitmap::VA_MAP, is_ours},
};

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

// TODO: Better realloc implementation
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void {
    if ptr.is_null() {
        return malloc(new_size);
    }

    if new_size > 1024 * 1024 * 1024 * 3 {
        *__errno_location() = NOMEM;
        return null_mut();
    }

    if !is_ours(ptr as usize) {
        return realloc_fallback(ptr, new_size);
    }

    if new_size == 0 {
        free(ptr);
        return malloc(1);
    }

    let mut raw_ptr = ptr;
    let mut offset: usize = 0;
    let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
    let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;

    if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
        let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
        if is_ours(presumed_original_ptr as usize) {
            raw_ptr = presumed_original_ptr;
            offset = (ptr as usize).wrapping_sub(raw_ptr as usize);
        }
    }

    let header = (raw_ptr as *mut OxHeader).sub(1);

    validate_ptr_for_abi(header);

    let raw_capacity;
    if (*header).class == 100 {
        raw_capacity = BIG_ALLOC_MAP
            .get(header as usize)
            .map(|meta| meta.size)
            .unwrap_or_else(|| {
                OxidallocError::AttackOrCorruption.log_and_abort(
                    header as *mut c_void,
                    "Missing big allocation metadata during realloc",
                    None,
                )
            });
    } else {
        raw_capacity = SIZE_CLASSES[(*header).class as usize];
    }

    let old_class = (*header).class;
    let new_class = match_size_class(new_size);
    let it = if old_class != 100 {
        ITERATIONS[old_class as usize]
    } else {
        1000
    };

    if let Some(new) = new_class {
        if old_class as usize == new && raw_capacity.saturating_sub(offset) >= new_size {
            return ptr;
        }
    }

    if old_class == 100 || it == 1 {
        let is_big = old_class == 100;
        let is_big_new = new_class.unwrap_or(100) == 100;

        let old_total = if is_big {
            align_to(raw_capacity + HEADER_SIZE, 4096)
        } else {
            align_to(raw_capacity + HEADER_SIZE, 16)
        };

        let size;

        let new_total = if is_big_new {
            size = new_size;
            align_to(new_size + HEADER_SIZE, 4096)
        } else {
            let new_class = match_size_class(new_size);
            if let Some(class) = new_class {
                size = SIZE_CLASSES[class];
                align_to(SIZE_CLASSES[class] + HEADER_SIZE, 16)
            } else {
                size = new_size;
                align_to(new_size + HEADER_SIZE, 4096)
            }
        };

        let new_class = match_size_class(size).unwrap_or(100) as u8;

        if new_total == old_total {
            (*header).class = new_class;
            if old_class == 100 && new_class != 100 {
                let _ = BIG_ALLOC_MAP.remove(header as usize);
            } else if new_class == 100 {
                BIG_ALLOC_MAP.insert(
                    header as usize,
                    BigAllocMeta {
                        size,
                        class: 100,
                        life_time: 0,
                        flags: 0,
                    },
                );
            }
            return ptr;
        }

        if new_total < old_total {
            let freed_start = align_to((header as usize) + new_total, 4096);
            let freed_len = old_total - new_total;

            if freed_start & 4095 == 0 && freed_len & 4095 == 0 {
                let is_ok = madvise(
                    freed_start as *mut c_void,
                    freed_len,
                    MadviseFlags::DONTNEED,
                )
                .is_ok();

                if is_ok {
                    let _ =
                        protect_memory(freed_start as *mut c_void, freed_len, RMProtFlags::NONE);

                    VA_MAP.free(freed_start, freed_len);

                    (*header).class = new_class;
                    if old_class == 100 && new_class != 100 {
                        let _ = BIG_ALLOC_MAP.remove(header as usize);
                    } else if new_class == 100 {
                        BIG_ALLOC_MAP.insert(
                            header as usize,
                            BigAllocMeta {
                                size,
                                class: 100,
                                life_time: 0,
                                flags: 0,
                            },
                        );
                    }
                };
            }

            return ptr;
        }

        if let Some(actual_new_va_size) = VA_MAP.realloc_inplace(
            header as usize,
            align_to(raw_capacity + HEADER_SIZE, 4096),
            align_to(size + HEADER_SIZE, 4096),
        ) {
            let grow_start = align_to((header as usize) + old_total, 4096);
            let grow_len = actual_new_va_size - old_total;

            // Use match for future debuging
            match protect_memory(
                grow_start as *mut c_void,
                grow_len,
                RMProtFlags::READ | RMProtFlags::WRITE,
            ) {
                Ok(_) => {
                    (*header).class = new_class;

                    if old_class == 100 && new_class != 100 {
                        let _ = BIG_ALLOC_MAP.remove(header as usize);
                    } else if new_class == 100 {
                        BIG_ALLOC_MAP.insert(
                            header as usize,
                            BigAllocMeta {
                                size,
                                class: 100,
                                life_time: 0,
                                flags: 0,
                            },
                        );
                    }

                    return ptr;
                }
                Err(_) => {
                    let rollback_start = (header as usize) + old_total;
                    let rollback_len = actual_new_va_size - old_total;

                    if rollback_start & 4095 == 0 && rollback_len & 4095 == 0 {
                        VA_MAP.free(rollback_start, rollback_len);
                    }
                }
            }
        }
    }

    let new_ptr = malloc(new_size);
    if new_ptr.is_null() {
        return std::ptr::null_mut();
    }

    let old_capacity = raw_capacity.saturating_sub(offset);
    std::ptr::copy_nonoverlapping(
        ptr as *const u8,
        new_ptr as *mut u8,
        old_capacity.min(new_size),
    );

    free(ptr);
    new_ptr
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn reallocarray(
    ptr: *mut c_void,
    nmemb: size_t,
    size: size_t,
) -> *mut c_void {
    let total_size = match nmemb.checked_mul(size) {
        Some(s) => s,
        None => {
            if let Some(errno_ptr) = __errno_location().as_mut() {
                *errno_ptr = NOMEM;
            }
            return null_mut();
        }
    };

    realloc(ptr, total_size)
}
