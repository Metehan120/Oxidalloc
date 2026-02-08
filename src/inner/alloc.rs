use std::{
    hint::{likely, unlikely},
    os::raw::c_void,
    ptr::{null_mut, write},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::{
    FREED_MAGIC, HEADER_SIZE, MAGIC, OX_CURRENT_STAMP, OX_TRIM, OxHeader,
    big_allocation::big_malloc,
    slab::{
        NUM_SIZE_CLASSES, SIZE_CLASSES, bulk_allocation::bulk_fill, get_size_4096_class,
        global::GlobalHandler, match_size_class, thread_local::ThreadLocalEngine,
    },
    trim::thread::spawn_gtrim_thread,
    va::{align_to, bootstrap::boot_strap, is_ours},
};
#[cfg(not(feature = "global-alloc"))]
use crate::{internals::__errno_location, sys::NOMEM};

static THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);
const BATCH_MIN: usize = 8;
const BATCH_MAX: usize = 32;
pub static BATCH_HINTS: [AtomicUsize; NUM_SIZE_CLASSES] =
    [const { AtomicUsize::new(32) }; NUM_SIZE_CLASSES];

pub const OFFSET_SIZE: usize = size_of::<usize>();
pub const TAG_SIZE: usize = OFFSET_SIZE * 2;
pub static TOTAL_MALLOC_FREE: AtomicUsize = AtomicUsize::new(0);
pub static mut HOT_READY: bool = false;

#[inline(always)]
unsafe fn try_split_from_icc(class: usize) -> *mut OxHeader {
    let class_4096 = get_size_4096_class();
    if class >= class_4096 {
        return null_mut();
    }

    let target_block = align_to(SIZE_CLASSES[class] + HEADER_SIZE, 16);

    for donor in (class + 1)..=class_4096 {
        let donor_block = align_to(SIZE_CLASSES[donor] + HEADER_SIZE, 16);
        let count = donor_block / target_block;

        if count < 2 || donor_block % target_block != 0 {
            continue;
        }

        let donor_header = GlobalHandler.pop_from_global(donor, 1);
        if donor_header.is_null() {
            continue;
        }

        let base = donor_header as *mut u8;
        let first = base as *mut OxHeader;

        write(
            first,
            OxHeader {
                next: null_mut(),
                class: class as u8,
                magic: FREED_MAGIC,
                life_time: OX_CURRENT_STAMP,
            },
        );

        let mut head_push = null_mut();
        let mut tail_push: *mut OxHeader = null_mut();

        for i in 1..count {
            let offset = i * target_block;
            let ptr = base.add(offset) as *mut OxHeader;

            write(
                ptr,
                OxHeader {
                    next: head_push,
                    class: class as u8,
                    magic: FREED_MAGIC,
                    life_time: OX_CURRENT_STAMP,
                },
            );

            if tail_push.is_null() {
                tail_push = ptr;
            }
            head_push = ptr;
        }

        if !head_push.is_null() {
            GlobalHandler.push_to_global(class, head_push, tail_push, count - 1);
        }

        return first;
    }

    null_mut()
}

#[cfg(not(feature = "global-alloc"))]
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

#[cold]
#[inline(never)]
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

    let split = try_split_from_icc(class);
    if !split.is_null() {
        return split;
    }

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

#[cold]
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
            if OX_TRIM {
                spawn_gtrim_thread();
            }
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

    #[cfg(feature = "hardened-malloc")]
    {
        (*cache).next = null_mut();
    }
    (*cache).magic = MAGIC;

    cache.add(1) as *mut c_void
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

    #[cfg(feature = "hardened-malloc")]
    {
        (*cache).next = null_mut();
    }
    (*cache).magic = MAGIC;

    cache.add(1) as *mut c_void
}

#[cold]
#[inline(never)]
pub unsafe fn allocate_cold(size: usize) -> *mut u8 {
    boot_strap();

    big_malloc(size)
}

#[inline(always)]
pub unsafe fn alloc_inner(size: usize) -> *mut c_void {
    if likely(size <= 4096 && size > 0) {
        let index = (size - 1) >> 4;
        let class = unsafe { *crate::slab::SIZE_LUT.get_unchecked(index) as usize };
        return if likely(HOT_READY) {
            allocate_hot(class)
        } else {
            allocate_boot_segment(class)
        };
    }

    if unlikely(size > 1024 * 1024 * 1024 * 3) {
        #[cfg(not(feature = "global-alloc"))]
        {
            *__errno_location() = NOMEM;
        }
        return null_mut();
    }

    if let Some(class) = match_size_class(size) {
        return if likely(HOT_READY) {
            return allocate_hot(class);
        } else {
            allocate_boot_segment(class)
        };
    }

    return allocate_cold(size) as *mut c_void;
}
