#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    alloc::Layout,
    os::raw::{c_int, c_void},
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use libc::{__errno_location, ENOMEM, size_t};

use crate::{
    HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OxHeader,
    abi::fallback::malloc_usable_size_fallback,
    big_allocation::big_malloc,
    slab::{
        ITERATIONS, SIZE_CLASSES, bulk_allocation::bulk_fill, global::GlobalHandler,
        match_size_class, thread_local::ThreadLocalEngine,
    },
    trim::{
        gtrim::GTrim,
        ptrim::PTrim,
        thread::{spawn_gtrim_thread, spawn_ptrim_thread},
    },
    va::{
        bootstrap::{SHUTDOWN, boot_strap},
        is_ours,
    },
};

static THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);
static ONCE: Once = Once::new();
const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;
pub static TOTAL_MALLOC_FREE: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
unsafe fn try_fill(thread: &ThreadLocalEngine, class: usize) -> *mut OxHeader {
    let mut output = null_mut();

    let batch = if class > 10 { ITERATIONS[class] } else { 16 };

    let global_cache = GlobalHandler.pop_from_global(thread.numa_node_id, class, batch);

    if !global_cache.is_null() {
        let mut tail = global_cache;
        let mut real = 1;

        // Loop through cache and found the last header and set linked list to null
        while real < batch
            && !(*tail).next.is_null()
            && is_ours((*tail).next as usize, Some(thread))
        {
            tail = (*tail).next;
            real += 1;
        }
        (*tail).next = null_mut();

        thread.push_to_thread_tailed(class, global_cache, tail, real);
        return thread.pop_from_thread(class);
    }

    for i in 0..3 {
        match bulk_fill(thread, class) {
            Ok(_) => {
                output = thread.pop_from_thread(class);
                break;
            }
            Err(_) => match i {
                2 => return null_mut(),
                _ => continue,
            },
        }
    }

    output
}

#[inline(always)]
// Separated allocation function for better scalability in future
unsafe fn allocate(layout: &Layout) -> *mut u8 {
    boot_strap();
    let size = layout.size();

    let class = match match_size_class(size) {
        Some(class) => class,
        None => return big_malloc(size),
    };

    let thread = ThreadLocalEngine::get_or_init();
    let mut cache = thread.pop_from_thread(class);

    // Check if cache is null
    if cache.is_null() {
        cache = try_fill(thread, class)
    }

    if cache.is_null() {
        return null_mut();
    }

    (*cache).next = null_mut();
    (*cache).magic = MAGIC;
    (*cache).in_use = 1;
    (*cache).used_before = 1;

    (cache as *mut u8).add(HEADER_SIZE)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: size_t) -> *mut c_void {
    if TOTAL_MALLOC_FREE.load(Ordering::Relaxed) < 256 {
        TOTAL_MALLOC_FREE.fetch_add(1, Ordering::Relaxed);
    } else {
        if !THREAD_SPAWNED.load(Ordering::Relaxed) {
            THREAD_SPAWNED.store(true, Ordering::Relaxed);
            ONCE.call_once(|| {
                spawn_ptrim_thread();
                spawn_gtrim_thread();
            });
        }
    }

    match Layout::array::<u8>(size) {
        Ok(layout) => allocate(&layout) as *mut c_void,
        Err(_) => {
            *__errno_location() = ENOMEM;
            null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> size_t {
    if ptr.is_null() {
        return 0;
    }

    let thread = ThreadLocalEngine::get_or_init();
    if !is_ours(ptr as usize, Some(thread)) {
        return malloc_usable_size_fallback(ptr);
    }

    let mut raw_ptr = ptr;
    let mut offset: usize = 0;
    let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
    let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;

    if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
        let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
        if is_ours(presumed_original_ptr as usize, Some(thread)) {
            raw_ptr = presumed_original_ptr;
            offset = (ptr as usize).wrapping_sub(raw_ptr as usize);
        }
    }

    let header = (raw_ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;

    if !is_ours(header as usize, Some(thread)) {
        return 0;
    }

    let size = (*header).size as usize;
    let raw_usable = match match_size_class(size) {
        Some(idx) => SIZE_CLASSES[idx],
        None => size,
    };
    raw_usable.saturating_sub(offset) as size_t
}

pub unsafe extern "C" fn malloc_trim(pad: size_t) -> c_int {
    let is_ok_p = PTrim.trim(pad);
    let is_ok_g = if is_ok_p.0 == 0 {
        let gtrim = GTrim.trim(pad);
        if gtrim.0 == 0 {
            if (is_ok_p.1 + gtrim.1) >= pad { 1 } else { 0 }
        } else {
            1
        }
    } else {
        1
    };

    if !SHUTDOWN.load(Ordering::Relaxed) && pad == 0 {
        1
    } else {
        is_ok_g | is_ok_p.0
    }
}
