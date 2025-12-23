#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use crate::{
    TOTAL_ALLOCATED, TOTAL_IN_USE,
    trim::{gtrim::GTrim, ptrim::PTrim},
    va::bootstrap::SHUTDOWN,
};

static TOTAL_TIME: AtomicUsize = AtomicUsize::new(0);
static TOTAL_TIME_GLOBAL: AtomicUsize = AtomicUsize::new(0);

unsafe fn decide_global() -> bool {
    if TOTAL_ALLOCATED.load(Ordering::Relaxed) == 0 {
        return false;
    }

    let total_allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);
    let total_in_use = TOTAL_IN_USE.load(Ordering::Relaxed);

    let in_use_percentage = if total_allocated > 0 {
        (total_in_use * 100) / total_allocated
    } else {
        0
    };

    let total = TOTAL_TIME_GLOBAL.load(Ordering::Relaxed);
    (in_use_percentage < 75 && total > 0) || (total % 25000 == 0 && total != 0)
}

unsafe fn decide_pthread() -> bool {
    if TOTAL_ALLOCATED.load(Ordering::Relaxed) == 0 {
        return false;
    }

    let total_allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);
    let total_in_use = TOTAL_IN_USE.load(Ordering::Relaxed);

    let in_use_percentage = if total_allocated > 0 {
        (total_in_use * 100) / total_allocated
    } else {
        0
    };

    let total = TOTAL_TIME.load(Ordering::Relaxed);
    (in_use_percentage < 70 && total > 0) || (total % 10000 == 0 && total != 0)
}

pub unsafe fn spawn_ptrim_thread() {
    std::thread::spawn(|| {
        while !SHUTDOWN.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            TOTAL_TIME.fetch_add(100, Ordering::Relaxed);

            if decide_pthread() {
                PTrim.trim();
            }
        }
    });
}

pub unsafe fn spawn_gtrim_thread() {
    std::thread::spawn(|| {
        while !SHUTDOWN.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            TOTAL_TIME_GLOBAL.fetch_add(100, Ordering::Relaxed);

            if decide_global() {
                GTrim.trim();
            }
        }
    });
}
