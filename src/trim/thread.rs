#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    time::Duration,
};

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, OX_CURRENT_STAMP, OX_TRIM_THRESHOLD, TOTAL_ALLOCATED, get_clock,
    trim::{TimeDecay, gtrim::GTrim},
    va::bootstrap::SHUTDOWN,
};

static TOTAL_TIME_GLOBAL: AtomicUsize = AtomicUsize::new(0);
pub static LAST_PRESSURE_CHECK: AtomicUsize = AtomicUsize::new(0);

pub static GLOBAL_DECAY: AtomicU8 = AtomicU8::new(0);
pub static PTRIM_DECAY: AtomicU8 = AtomicU8::new(0);

unsafe fn decide_global(decay: &TimeDecay) -> bool {
    if TOTAL_ALLOCATED.load(Ordering::Relaxed) == 0 {
        return false;
    }

    if (OX_TRIM_THRESHOLD.load(Ordering::Relaxed) < decay.get_threshold() as usize)
        && (OX_TRIM_THRESHOLD.load(Ordering::Relaxed) != 0)
    {
        OX_TRIM_THRESHOLD.store(decay.get_threshold() as usize, Ordering::Relaxed);
    }

    let total = TOTAL_TIME_GLOBAL.load(Ordering::Relaxed);
    if total % 300 == 0 {
        LAST_PRESSURE_CHECK.store(check_memory_pressure(), Ordering::Relaxed);
    }

    if LAST_PRESSURE_CHECK.load(Ordering::Relaxed) > 85 {
        return true;
    }

    let timing = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed);
    total % timing == 0 && total != 0
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

pub unsafe fn spawn_gtrim_thread() {
    std::thread::spawn(|| {
        while !SHUTDOWN.load(Ordering::Acquire) {
            let decay = TimeDecay::from_u8(GLOBAL_DECAY.load(Ordering::Relaxed));
            std::thread::sleep(Duration::from_millis(decay.get_trim_time()));

            TOTAL_TIME_GLOBAL.fetch_add(decay.get_trim_time() as usize, Ordering::Relaxed);

            let time = get_clock().elapsed().as_millis() as usize;
            OX_CURRENT_STAMP = time;

            if decide_global(&decay) {
                GTrim.trim(OX_TRIM_THRESHOLD.load(Ordering::Relaxed));
            }
        }
    });
}
