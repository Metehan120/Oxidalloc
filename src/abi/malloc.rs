use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use libc::size_t;

use crate::{
    Err, HEADER_SIZE, MAGIC, OX_CURRENT_STAMP, OxHeader, OxidallocError, TOTAL_IN_USE, TOTAL_OPS,
    big_allocation::big_malloc,
    get_clock,
    slab::{
        SIZE_CLASSES, bulk_allocation::bulk_fill, global::GlobalHandler, match_size_class,
        thread_local::ThreadLocalEngine,
    },
    va::{
        bootstrap::{VA_LEN, boot_strap},
        va_helper::is_ours,
    },
};

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: size_t) -> *mut c_void {
    unsafe {
        boot_strap();

        let total = TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut stamp: usize = 0;
        if total > 0 && total % 1500 == 0 {
            let time = get_clock().elapsed().as_millis() as usize;
            stamp = OX_CURRENT_STAMP.swap(time, Ordering::Relaxed);
        }

        if size > VA_LEN.load(Ordering::Relaxed) {
            return null_mut();
        }

        let class = match match_size_class(size) {
            Some(class) => class,
            None => return big_malloc(size),
        };

        let thread = ThreadLocalEngine::get_or_init();
        let mut cache = thread.pop_from_thread(class);

        if cache.is_null() {
            let global_cache = GlobalHandler.pop_batch_from_global(class, 1);

            if !global_cache.is_null() {
                cache = global_cache;
            } else {
                for i in 0..3 {
                    match bulk_fill(thread, class) {
                        Ok(_) => {
                            cache = thread.pop_from_thread(class);
                            break;
                        }
                        Err(error) => {
                            match error {
                                Err::OutOfMemory => (),
                                Err::OutOfReservation => OxidallocError::VaBitmapExhausted
                                    .log_and_abort(
                                        null_mut(),
                                        "VA bitmap exhausted | This is expected",
                                        None,
                                    ),
                            }

                            if i == 2 {
                                return null_mut();
                            }
                        }
                    }
                }
            }
        }

        if cache.is_null() {
            return null_mut();
        }

        (*cache).next = null_mut();
        (*cache).magic = MAGIC;
        (*cache).in_use = 1;
        (*cache).life_time = stamp;

        TOTAL_IN_USE.fetch_add(1, Ordering::Relaxed);
        (cache as *mut u8).add(HEADER_SIZE) as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc_usable_size(ptr: *mut c_void) -> size_t {
    unsafe {
        if ptr.is_null() {
            return 0;
        }

        if !is_ours(ptr as usize) {
            return 0;
        }

        let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

        if !is_ours(header as usize) {
            return 0;
        }

        let magic = std::ptr::read_volatile(&(*header).magic);

        if magic == MAGIC {
            let size = (*header).size as usize;
            match match_size_class(size) {
                Some(idx) => SIZE_CLASSES[idx],
                None => size,
            }
        } else {
            (*header).size as usize
        }
    }
}
