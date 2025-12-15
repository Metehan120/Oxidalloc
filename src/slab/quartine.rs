use std::{
    ptr::null_mut,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::OxidallocError;

pub static MAX_QUARANTINE: usize = 1024 * 1024;

pub static QUARANTINE: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_QUARANTINED: AtomicUsize = AtomicUsize::new(0);

pub fn quarantine(ptr: usize) {
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
}
