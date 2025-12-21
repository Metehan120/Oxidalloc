#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use crate::{TOTAL_ALLOCATED, TOTAL_IN_USE, trim::trim_pthread::PTrim, va::bootstrap::SHUTDOWN};

static TOTAL_TIME: AtomicUsize = AtomicUsize::new(0);

unsafe fn decide() -> bool {
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

    in_use_percentage < 75
        || check_memory_pressure() > 75
        || TOTAL_TIME.load(Ordering::Relaxed) % 10000 == 0
}

unsafe fn check_memory_pressure() -> usize {
    unsafe {
        let mut info: libc::sysinfo = std::mem::zeroed();

        if libc::sysinfo(&mut info) != 0 {
            return 50;
        }

        let total_ram = info.totalram as usize;
        let free_ram = info.freeram as usize;
        let total_swap = info.totalswap as usize;
        let free_swap = info.freeswap as usize;

        let total_available = free_ram + free_swap;
        let total_memory = total_ram + total_swap;

        if total_memory == 0 {
            return 50;
        }

        let used = total_memory.saturating_sub(total_available);
        (used * 100) / total_memory
    }
}

pub unsafe fn spawn_trim_thread() {
    std::thread::spawn(|| {
        while !SHUTDOWN.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            TOTAL_TIME.fetch_add(100, Ordering::Relaxed);

            if decide() {
                PTrim.trim();
            }
        }
    });
}
