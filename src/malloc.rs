use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use libc::{madvise, size_t};

use crate::{
    FLAG_FREED, FLAG_NON, HEADER_SIZE, MAP, OX_CURRENT_STAMP, OxHeader, OxidallocError, PROT,
    TOTAL_IN_USE, TOTAL_OPS,
    free::is_ours,
    get_clock,
    global::GlobalHandler,
    internals::{
        AllocationHelper, MAGIC, SIZE_CLASSES, VA_END, VA_MAP, VA_START, align, bootstrap,
    },
    thread_local::ThreadLocalEngine,
};

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: size_t) -> *mut c_void {
    unsafe {
        bootstrap();

        if size + HEADER_SIZE > VA_END.load(Ordering::Relaxed) - VA_START.load(Ordering::Relaxed) {
            return null_mut();
        }

        let total = TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if total > 0 && total % 1000 == 0 {
            let stamp = get_clock().elapsed().as_millis() as usize;

            OX_CURRENT_STAMP.store(stamp, std::sync::atomic::Ordering::Relaxed);
        }

        let class = match AllocationHelper.match_size_class(size) {
            Some(class) => class,
            None => {
                let total = size + HEADER_SIZE;

                let aligned_total = align(total);
                let hint = match VA_MAP.alloc(aligned_total) {
                    Some(hint) => hint,
                    None => return null_mut(),
                };

                let allocated = libc::mmap(
                    hint as *mut c_void,
                    aligned_total,
                    PROT,
                    MAP | libc::MAP_FIXED,
                    -1,
                    0,
                );

                if allocated == libc::MAP_FAILED {
                    return null_mut();
                }

                madvise(allocated, aligned_total, libc::MADV_HUGEPAGE);

                let header = allocated as *mut OxHeader;
                (*header).magic = MAGIC;
                (*header).next = null_mut();
                (*header).size = size as u64;
                (*header).flag = FLAG_NON;
                (*header).in_use.store(1, Ordering::Relaxed);

                return (header as *mut u8).add(HEADER_SIZE) as *mut c_void;
            }
        };

        let engine = ThreadLocalEngine::get_or_init();
        let mut popped = engine.pop_from_thread(class);
        let current = OX_CURRENT_STAMP.load(Ordering::Relaxed);

        if popped.is_null() {
            let global = GlobalHandler.pop_batch_from_global(class, 16);

            if !global.is_null() {
                if (*global).flag != FLAG_FREED {
                    engine.push_to_thread(class, global);
                    let ptr = engine.pop_from_thread(class);
                    engine.usages[class].fetch_add(16, Ordering::Relaxed);

                    (*ptr).life_time = current;
                    (*ptr).next = null_mut();
                    (*ptr).magic = MAGIC;
                    (*ptr).in_use.store(1, Ordering::Relaxed);

                    TOTAL_IN_USE.fetch_add(1, Ordering::Relaxed);

                    engine.usages[class].fetch_sub(1, Ordering::Relaxed);
                    return (ptr as *mut u8).add(HEADER_SIZE) as *mut c_void;
                }
            }

            for i in 0..3 {
                if AllocationHelper.bulk_allocate(class) {
                    break;
                }

                if i == 2 {
                    return null_mut();
                }
            }

            let popped_2 = engine.pop_from_thread(class);
            popped = popped_2;
        }

        if popped.is_null() {
            OxidallocError::OutOfMemory
                .log_and_abort(popped as *mut c_void, "Not able to allocate memory")
        }

        if (*popped).flag != FLAG_FREED {
            (*popped).flag = FLAG_NON;
            (*popped).life_time = current;
            (*popped).next = null_mut();
            (*popped).magic = MAGIC;
            (*popped).in_use.store(1, Ordering::Relaxed);

            TOTAL_IN_USE.fetch_add(1, Ordering::Relaxed);

            engine.usages[class].fetch_sub(1, Ordering::Relaxed);
            (popped as *mut u8).add(HEADER_SIZE) as *mut c_void
        } else {
            return malloc(size);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc_usable_size(ptr: *mut c_void) -> size_t {
    unsafe {
        if ptr.is_null() {
            return 0;
        }

        if !is_ours(ptr) {
            return 0;
        }

        let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

        if !is_ours(header as *mut c_void) {
            return 0;
        }

        let magic = std::ptr::read_volatile(&(*header).magic);

        if magic == MAGIC {
            let size = (*header).size as usize;
            match AllocationHelper.match_size_class(size) {
                Some(idx) => SIZE_CLASSES[idx],
                None => size,
            }
        } else {
            (*header).size as usize
        }
    }
}
