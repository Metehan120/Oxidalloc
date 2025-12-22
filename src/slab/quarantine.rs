use std::{
    ptr::{null_mut, read_volatile},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{MAGIC, OxidallocError, slab::thread_local::ThreadLocalEngine, va::va_helper::is_ours};

pub static MAX_QUARANTINE: usize = 1024 * 1024 * 10;

pub static QUARANTINE: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_QUARANTINED: AtomicUsize = AtomicUsize::new(0);

pub fn quarantine(thread_cache: Option<&ThreadLocalEngine>, ptr: usize, class: usize) -> bool {
    // Try recovering header from old popped head, if available
    //
    // How is this works:
    // - Malloc is fast but atomic so we can actually recover data if timing is right.
    // - Try reading next header of cache to see if it's ours
    //
    // Better trying to recover header from old popped head, if available
    // If not recoverable than then leak it
    if let Some(healing_cache) = thread_cache {
        healing_cache.lock(class);
        let candidate = healing_cache.latest_popped_next[class].load(Ordering::Relaxed);

        if !candidate.is_null() && is_ours(candidate as usize) {
            let latest_usage = healing_cache.latest_usages[class].load(Ordering::Relaxed);

            unsafe {
                let magic = read_volatile(&(*candidate).magic);
                if magic == MAGIC || magic == 0 {
                    healing_cache.cache[class].store(candidate, Ordering::Relaxed);
                    healing_cache.usages[class].store(latest_usage, Ordering::Relaxed);
                    healing_cache.unlock(class);
                    return true;
                }
                healing_cache.unlock(class);
            }
        }
    }

    let guard = QUARANTINE.load(Ordering::Relaxed);
    TOTAL_QUARANTINED.fetch_add(1, Ordering::Relaxed);

    if guard == ptr {
        OxidallocError::DoubleQuarantine.log_and_abort(
            null_mut(),
            "Double quarantine detected, aborting process",
            None,
        )
    }

    if TOTAL_QUARANTINED.load(Ordering::Relaxed) < MAX_QUARANTINE {
        QUARANTINE.swap(ptr, Ordering::AcqRel);
    } else {
        OxidallocError::TooMuchQuarantine.log_and_abort(
            null_mut(),
            "Too much quarantine detected, aborting process",
            None,
        )
    }

    false
}
