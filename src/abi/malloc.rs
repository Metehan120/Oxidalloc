#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    alloc::Layout,
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
};

use libc::{__errno_location, ENOMEM, size_t};

use crate::{
    Err, HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OX_CURRENT_STAMP, OxHeader, OxidallocError,
    TOTAL_IN_USE, TOTAL_OPS,
    big_allocation::big_malloc,
    get_clock,
    slab::{
        ITERATIONS, SIZE_CLASSES, bulk_allocation::bulk_fill, global::GlobalHandler,
        match_size_class, thread_local::ThreadLocalEngine,
    },
    trim::thread::{spawn_gtrim_thread, spawn_ptrim_thread},
    va::{
        bootstrap::{VA_LEN, boot_strap},
        va_helper::is_ours,
    },
};

static THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);
static ONCE: Once = Once::new();
const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;

#[inline(always)]
// Separated allocation function for better scalability in future
unsafe fn allocate(layout: Layout) -> *mut u8 {
    boot_strap();
    let size = layout.size();

    let total = TOTAL_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if total >= 10000 {
        if !THREAD_SPAWNED.load(Ordering::Relaxed) {
            THREAD_SPAWNED.store(true, Ordering::Relaxed);
            ONCE.call_once(|| {
                spawn_ptrim_thread();
                spawn_gtrim_thread();
            });
        }
    }

    if total > 0 && total % 1500 == 0 {
        let time = get_clock().elapsed().as_millis() as usize;
        OX_CURRENT_STAMP.swap(time, Ordering::Relaxed);
    }

    if size > VA_LEN.load(Ordering::Relaxed) {
        *__errno_location() = ENOMEM;
        return null_mut();
    }

    let class = match match_size_class(size) {
        Some(class) => class,
        None => return big_malloc(size),
    };

    let thread = ThreadLocalEngine::get_or_init();
    let mut cache = thread.pop_from_thread(class);

    // Check if cache is null
    if cache.is_null() {
        // Check global cache if its allocated then pop batch from global cache
        let batch = if class > 10 { ITERATIONS[class] } else { 16 };
        let global_cache = GlobalHandler.pop_batch_from_global(class, batch);

        if !global_cache.is_null() {
            let mut tail = global_cache;
            let mut real = 1;

            // Loop through cache and found the last header and set linked list to null
            while real < batch && !(*tail).next.is_null() && is_ours((*tail).next as usize) {
                tail = (*tail).next;
                real += 1;
            }
            (*tail).next = null_mut();

            thread.push_to_thread_tailed(class, global_cache, tail, real);
            cache = thread.pop_from_thread(class);
        } else {
            for i in 0..3 {
                // Global is null, try to fill thread cache
                match bulk_fill(thread, class) {
                    // If fill succeeds, pop from thread cache
                    Ok(_) => {
                        cache = thread.pop_from_thread(class);
                        break;
                    }
                    // If fill fails, check error if VA exhausted abort
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

    TOTAL_IN_USE.fetch_add(1, Ordering::Relaxed);
    (cache as *mut u8).add(HEADER_SIZE)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: size_t) -> *mut c_void {
    match Layout::array::<u8>(size) {
        Ok(layout) => allocate(layout) as *mut c_void,
        Err(_) => null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> size_t {
    if ptr.is_null() {
        return 0;
    }

    if !is_ours(ptr as usize) {
        return 0;
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

    let header = (raw_ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

    if !is_ours(header as usize) {
        return 0;
    }

    let size = (*header).size as usize;
    let raw_usable = match match_size_class(size) {
        Some(idx) => SIZE_CLASSES[idx],
        None => size,
    };
    raw_usable.saturating_sub(offset) as size_t
}
