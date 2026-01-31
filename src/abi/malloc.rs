use std::{
    hint::{likely, unlikely},
    os::raw::{c_int, c_void},
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::{
    HEADER_SIZE, MAGIC, OX_ALIGN_TAG, OxHeader, OxidallocError,
    abi::fallback::malloc_usable_size_fallback,
    big_allocation::big_malloc,
    internals::{__errno_location, hashmap::BIG_ALLOC_MAP, size_t},
    slab::{
        NUM_SIZE_CLASSES, SIZE_CLASSES, bulk_allocation::bulk_fill, global::GlobalHandler,
        match_size_class, thread_local::ThreadLocalEngine,
    },
    sys::NOMEM,
    trim::{gtrim::GTrim, thread::spawn_gtrim_thread},
    va::{bootstrap::boot_strap, is_ours},
};

static THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);
const BATCH_MIN: usize = 8;
const BATCH_MAX: usize = 32;
pub static BATCH_HINTS: [AtomicUsize; NUM_SIZE_CLASSES] =
    [const { AtomicUsize::new(32) }; NUM_SIZE_CLASSES];

const OFFSET_SIZE: usize = size_of::<usize>();
const TAG_SIZE: usize = OFFSET_SIZE * 2;
pub static TOTAL_MALLOC_FREE: AtomicUsize = AtomicUsize::new(0);
pub static mut HOT_READY: bool = false;

pub(crate) fn reset_fork_thread_state() {
    THREAD_SPAWNED.store(false, Ordering::Relaxed);
    TOTAL_MALLOC_FREE.store(1024, Ordering::Relaxed);
    unsafe {
        HOT_READY = false;
    }
}

#[cfg(feature = "hardened-malloc")]
#[inline(always)]
pub(crate) unsafe fn validate_ptr(ptr: *mut OxHeader) -> u64 {
    use crate::FREED_MAGIC;
    use std::ptr::read_volatile;

    let magic = read_volatile(&(*ptr).magic);
    if unlikely(magic != MAGIC && magic != FREED_MAGIC) {
        OxidallocError::AttackOrCorruption.log_and_abort(
            null_mut() as *mut c_void,
            "Attack or corruption detected; aborting process. External system access and RAM module checks recommended.",
            None,
        )
    }
    magic
}

#[inline(always)]
unsafe fn try_fill(thread: &mut ThreadLocalEngine, class: usize) -> *mut OxHeader {
    let mut output = null_mut();

    let batch = BATCH_HINTS[class]
        .load(Ordering::Relaxed)
        .clamp(BATCH_MIN, BATCH_MAX);

    let global_cache = GlobalHandler.pop_from_global(class, batch);

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

        if real == batch {
            bump_batch_hint(class, true);
        } else {
            bump_batch_hint(class, false);
        }

        return thread.pop_from_thread(class);
    }

    bump_batch_hint(class, false);

    for i in 0..3 {
        match bulk_fill(thread, class) {
            Ok(_) => {
                output = thread.pop_from_thread(class);
                bump_batch_hint(class, true);
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
fn bump_batch_hint(class: usize, up: bool) {
    let _ = BATCH_HINTS[class].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |val| {
        let cur = val.clamp(BATCH_MIN, BATCH_MAX);
        let next = if up {
            (cur + 1).min(BATCH_MAX)
        } else {
            cur.saturating_sub(2).max(BATCH_MIN)
        };
        Some(next)
    });
}

#[inline(always)]
unsafe fn allocate_hot(class: usize) -> *mut c_void {
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

    cache.add(1) as *mut c_void
}

#[inline(always)]
// Separated allocation function for better scalability in future
unsafe fn allocate_boot_segment(class: usize) -> *mut c_void {
    boot_strap();

    if TOTAL_MALLOC_FREE.load(Ordering::Relaxed) < 1024 {
        TOTAL_MALLOC_FREE.fetch_add(1, Ordering::Relaxed);
    } else {
        if THREAD_SPAWNED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            spawn_gtrim_thread();
            HOT_READY = true;
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

    cache.add(1) as *mut c_void
}

#[cold]
pub unsafe fn allocate_cold(size: usize) -> *mut u8 {
    boot_strap();

    big_malloc(size)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: size_t) -> *mut c_void {
    if likely(size <= 4096 && size > 0) {
        let index = (size - 1) >> 4;
        let class = unsafe { *crate::slab::SIZE_LUT.get_unchecked(index) as usize };
        if likely(HOT_READY) {
            return allocate_hot(class);
        }
    }

    if unlikely(size > 1024 * 1024 * 1024 * 3) {
        *__errno_location() = NOMEM;
        return null_mut();
    }

    if let Some(class) = match_size_class(size) {
        if likely(HOT_READY) {
            return allocate_hot(class);
        }
        return allocate_boot_segment(class);
    }

    return allocate_cold(size) as *mut c_void;
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

    let mut size = (*header).class as usize;
    if size == 100 {
        let payload_size = BIG_ALLOC_MAP
            .get(header as usize)
            .map(|meta| meta.size)
            .unwrap_or_else(|| {
                OxidallocError::AttackOrCorruption.log_and_abort(
                    header as *mut c_void,
                    "Missing big allocation metadata during malloc_usable_size",
                    None,
                )
            });

        size = payload_size;
    } else {
        size = SIZE_CLASSES[size]
    }

    let raw_usable = match match_size_class(size) {
        Some(idx) => SIZE_CLASSES[idx],
        None => size,
    };
    raw_usable.saturating_sub(offset) as size_t
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_trim(pad: size_t) -> c_int {
    let is_ok_g = GTrim.trim(pad);

    is_ok_g.0
}
