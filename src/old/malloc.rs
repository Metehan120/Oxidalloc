use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use libc::{madvise, pthread_self, size_t};

use crate::{
    FLAG_NON, HEADER_SIZE, MAP, OX_CURRENT_STAMP, OxHeader, OxidallocError, PROT, TOTAL_IN_USE,
    TOTAL_OPS,
    free::is_ours,
    get_clock,
    global::GlobalHandler,
    internals::{
        AllocationHelper, MAGIC, SIZE_CLASSES, VA_END, VA_MAP, VA_START, align_to, bootstrap,
    },
    thread_local::ThreadLocalEngine,
};

#[inline]
pub fn current_thread_id() -> u32 {
    unsafe {
        let tid = pthread_self() as u64;
        (tid ^ (tid >> 32)) as u32
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: size_t) -> *mut c_void {
    unsafe {
        bootstrap();

        let size = match size {
            0 => 1,
            _ => size,
        };

        let va_len = VA_END
            .load(Ordering::Relaxed)
            .saturating_sub(VA_START.load(Ordering::Relaxed));

        let total_size = match size.checked_add(HEADER_SIZE) {
            Some(total_size) => total_size,
            None => return null_mut(),
        };

        if total_size > va_len {
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
                let aligned_total = align_to(total_size, 4096);
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
        let mut is_general_ok = false;

        if popped.is_null() {
            let global = GlobalHandler.pop_batch_from_global(class, 16);

            if !global.is_null() {
                let mut tail = global;
                let mut real = 1;
                while real < 16 && !(*tail).next.is_null() && is_ours((*tail).next as *mut c_void) {
                    tail = (*tail).next;
                    real += 1;
                }
                (*tail).next = null_mut();
                engine.push_to_thread_tailed(class, global, tail);
                engine.usages[class].fetch_add(real, Ordering::Relaxed);
                popped = engine.pop_from_thread(class);

                if popped.is_null() {
                    OxidallocError::MemoryCorruption.log_and_abort(
                        global as *mut c_void,
                        "Global list corrupted after push_to_thread",
                    );
                }

                is_general_ok = true;
            }

            if !is_general_ok {
                for i in 0..3 {
                    if AllocationHelper.bulk_allocate(class, engine) {
                        break;
                    }

                    if i == 2 {
                        return null_mut();
                    }
                }

                let popped_2 = engine.pop_from_thread(class);
                popped = popped_2;
            }
        }

        if popped.is_null() {
            OxidallocError::OutOfMemory
                .log_and_abort(popped as *mut c_void, "Not able to allocate memory")
        }

        (*popped).flag = FLAG_NON;
        (*popped).life_time = current;
        (*popped).next = null_mut();
        (*popped).magic = MAGIC;
        (*popped).in_use.store(1, Ordering::Relaxed);
        (*popped).thread_id = current_thread_id();

        TOTAL_IN_USE.fetch_add(1, Ordering::Relaxed);
        engine.usages[class].fetch_sub(1, Ordering::Relaxed);
        (popped as *mut u8).add(HEADER_SIZE) as *mut c_void
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
