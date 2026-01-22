use std::{
    alloc::Layout,
    hint::{likely, unlikely},
    os::raw::{c_int, c_void},
    ptr::{null_mut, read_volatile},
    sync::{
        Once,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use libc::{__errno_location, ENOMEM, size_t};

use crate::{
    FREED_MAGIC, HAS_ALIGNED_PAGES, HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OxHeader, OxidallocError,
    abi::fallback::malloc_usable_size_fallback,
    big_allocation::big_malloc,
    slab::{
        SIZE_CLASSES, bulk_allocation::bulk_fill, global::GlobalHandler, match_size_class,
        thread_local::ThreadLocalEngine,
    },
    trim::{gtrim::GTrim, thread::spawn_gtrim_thread},
    va::{
        bootstrap::{IS_BOOTSTRAP, SHUTDOWN, boot_strap},
        is_ours,
    },
};

static THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);
static ONCE: Once = Once::new();
const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;
pub static TOTAL_MALLOC_FREE: AtomicUsize = AtomicUsize::new(0);
pub static mut HOT_READY: bool = false;

#[inline(always)]
pub(crate) unsafe fn validate_ptr(ptr: *mut OxHeader) {
    let magic = read_volatile(&(*ptr).magic);

    if unlikely(magic != MAGIC && magic != FREED_MAGIC) {
        OxidallocError::AttackOrCorruption.log_and_abort(
            null_mut() as *mut c_void,
            "Attack or corruption detected; aborting process. External system access and RAM module checks recommended.",
            None,
        )
    }
}

#[cold]
#[inline(never)]
unsafe fn try_fill(thread: &mut ThreadLocalEngine, class: usize) -> *mut OxHeader {
    let mut output = null_mut();

    let batch = 32;

    let global_cache = GlobalHandler.pop_from_global(thread.numa_node_id, class, batch);

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
unsafe fn allocate_hot(class: usize) -> *mut OxHeader {
    let thread = ThreadLocalEngine::get_or_init();
    let mut cache = thread.pop_from_thread(class);

    // Check if cache is null
    if unlikely(cache.is_null()) {
        cache = try_fill(thread, class);

        if cache.is_null() {
            return null_mut();
        }
    }

    #[cfg(feature = "hardened-malloc")]
    {
        if unlikely(!is_ours(cache as usize)) {
            OxidallocError::AttackOrCorruption.log_and_abort(
                null_mut() as *mut c_void,
                "Attack or corruption detected; aborting process. External system access and RAM module checks recommended.",
                None,
            )
        }
    }

    #[cfg(feature = "hardened-malloc")]
    validate_ptr(cache);

    (*cache).next = null_mut();
    (*cache).magic = MAGIC;

    cache.add(1)
}

#[inline(always)]
// Separated allocation function for better scalability in future
unsafe fn allocate_boot_segment(class: usize) -> *mut OxHeader {
    if unlikely(IS_BOOTSTRAP.load(Ordering::Relaxed)) {
        boot_strap();
    }

    if TOTAL_MALLOC_FREE.load(Ordering::Relaxed) < 1024 {
        TOTAL_MALLOC_FREE.fetch_add(1, Ordering::Relaxed);
    } else {
        if !THREAD_SPAWNED.load(Ordering::Relaxed) {
            THREAD_SPAWNED.store(true, Ordering::Relaxed);
            ONCE.call_once(|| {
                spawn_gtrim_thread();
                HOT_READY = true;
            });
        }
    }

    let thread = ThreadLocalEngine::get_or_init();
    let mut cache = thread.pop_from_thread(class);

    // Check if cache is null
    if unlikely(cache.is_null()) {
        cache = try_fill(thread, class);

        if cache.is_null() {
            return null_mut();
        }
    }

    #[cfg(feature = "hardened-malloc")]
    {
        if unlikely(!is_ours(cache as usize)) {
            OxidallocError::AttackOrCorruption.log_and_abort(
                null_mut() as *mut c_void,
                "Attack or corruption detected; aborting process. External system access and RAM module checks highly recommended.",
                None,
            )
        }
    }

    #[cfg(feature = "hardened-malloc")]
    validate_ptr(cache);

    (*cache).next = null_mut();
    (*cache).magic = MAGIC;

    cache.add(1)
}

#[cold]
pub unsafe fn allocate_cold(size: usize) -> *mut u8 {
    if unlikely(IS_BOOTSTRAP.load(Ordering::Relaxed)) {
        boot_strap();
    }

    big_malloc(size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: size_t) -> *mut c_void {
    fall_back_to_slow(size)
}

#[inline(always)]
unsafe fn fall_back_to_slow(size: size_t) -> *mut c_void {
    if unlikely(size > 1024 * 1024 * 1024 * 3) {
        *__errno_location() = ENOMEM;
        return null_mut();
    }

    if let Ok(layout) = Layout::array::<u8>(size) {
        if let Some(class) = match_size_class(layout.size()) {
            if likely(HOT_READY) {
                return allocate_hot(class) as *mut c_void;
            } else {
                return allocate_boot_segment(class) as *mut c_void;
            }
        }

        return allocate_cold(size) as *mut c_void;
    }

    *__errno_location() = ENOMEM;
    null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> size_t {
    if ptr.is_null() {
        return 0;
    }

    if !is_ours(ptr as usize) {
        return malloc_usable_size_fallback(ptr);
    }

    let mut raw_ptr = ptr;
    let mut offset: usize = 0;
    if HAS_ALIGNED_PAGES.load(Ordering::Relaxed) {
        let tag_loc = (ptr as usize).wrapping_sub(TAG_SIZE) as *const usize;
        let raw_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *const usize;

        if std::ptr::read_unaligned(tag_loc) == OX_ALIGN_TAG {
            let presumed_original_ptr = std::ptr::read_unaligned(raw_loc) as *mut c_void;
            if is_ours(presumed_original_ptr as usize) {
                raw_ptr = presumed_original_ptr;
                offset = (ptr as usize).wrapping_sub(raw_ptr as usize);
            }
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_trim(pad: size_t) -> c_int {
    let is_ok_g = GTrim.trim(pad);

    if !SHUTDOWN.load(Ordering::Relaxed) && pad == 0 {
        1
    } else {
        is_ok_g.0
    }
}
