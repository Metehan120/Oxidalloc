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
    if let Some(cache) = thread_cache {
        let detached = cache.tls[class].head.swap(null_mut(), Ordering::Acquire);

        if detached.is_null() {
            return false;
        }

        let candidate = cache.tls[class].latest_next.load(Ordering::Relaxed);
        if candidate.is_null() || !is_ours(candidate as usize) {
            return false;
        }

        unsafe {
            let magic = read_volatile(&(*candidate).magic);
            if magic != MAGIC && magic != 0 {
                return false;
            }
        }

        let usage = cache.tls[class].latest_usage.load(Ordering::Relaxed);

        if cache.tls[class]
            .head
            .compare_exchange(null_mut(), candidate, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            cache.tls[class].usage.store(usage, Ordering::Relaxed);
            return true;
        }

        return false;
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
