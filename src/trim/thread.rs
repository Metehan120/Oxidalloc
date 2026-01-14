#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, AVERAGE_BLOCK_TIMES_PTHREAD, OX_CURRENT_STAMP, OX_TRIM_THRESHOLD,
    TOTAL_ALLOCATED, get_clock,
    trim::{gtrim::GTrim, ptrim::PTrim},
    va::bootstrap::SHUTDOWN,
};

static TOTAL_TIME: AtomicUsize = AtomicUsize::new(0);
static TOTAL_TIME_GLOBAL: AtomicUsize = AtomicUsize::new(0);
static LAST_PRESSURE_CHECK: AtomicUsize = AtomicUsize::new(0);

unsafe fn decide_global() -> bool {
    if TOTAL_ALLOCATED.load(Ordering::Relaxed) == 0 {
        return false;
    }

    let total = TOTAL_TIME_GLOBAL.load(Ordering::Relaxed);
    if total % 300 == 0 {
        LAST_PRESSURE_CHECK.store(check_memory_pressure(), Ordering::Relaxed);
    }

    if LAST_PRESSURE_CHECK.load(Ordering::Relaxed) > 75 {
        return true;
    }

    let timing = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed);
    total % timing == 0 && total != 0
}

unsafe fn decide_pthread() -> bool {
    if TOTAL_ALLOCATED.load(Ordering::Relaxed) == 0 {
        return false;
    }

    let total = TOTAL_TIME.load(Ordering::Relaxed);
    if total % 400 == 0 {
        LAST_PRESSURE_CHECK.store(check_memory_pressure(), Ordering::Relaxed);
    }

    let pressure = LAST_PRESSURE_CHECK.load(Ordering::Relaxed);
    if pressure > 85 {
        return true;
    }

    let pthread_avg = AVERAGE_BLOCK_TIMES_PTHREAD.load(Ordering::Relaxed);
    let global_avg = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed);

    let tls_is_worse = pthread_avg + 10 > global_avg;

    let timing = pthread_avg.max(1);
    let periodic = total % timing == 0;

    periodic && tls_is_worse
}

fn check_memory_pressure() -> usize {
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

pub unsafe fn spawn_ptrim_thread() {
    std::thread::spawn(|| {
        while !SHUTDOWN.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            TOTAL_TIME.fetch_add(100, Ordering::Relaxed);

            let time = get_clock().elapsed().as_millis() as usize;
            OX_CURRENT_STAMP.store(time, Ordering::Relaxed);

            if decide_pthread() {
                PTrim.trim(OX_TRIM_THRESHOLD.load(Ordering::Relaxed));
            }
        }
    });
}

pub unsafe fn spawn_gtrim_thread() {
    std::thread::spawn(|| {
        while !SHUTDOWN.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            TOTAL_TIME_GLOBAL.fetch_add(100, Ordering::Relaxed);

            let time = get_clock().elapsed().as_millis() as usize;
            OX_CURRENT_STAMP.store(time, Ordering::Relaxed);

            if decide_global() {
                GTrim.trim(OX_TRIM_THRESHOLD.load(Ordering::Relaxed));
            }
        }
    });
}
