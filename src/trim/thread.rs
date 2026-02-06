use std::{
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    time::Duration,
};

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, OX_CURRENT_STAMP, OX_TRIM_THRESHOLD, get_clock,
    trim::{TimeDecay, gtrim::GTrim},
};

static TOTAL_TIME_GLOBAL: AtomicUsize = AtomicUsize::new(0);
static LAST_TRIM_GLOBAL: AtomicUsize = AtomicUsize::new(0);
pub static LAST_PRESSURE_CHECK: AtomicUsize = AtomicUsize::new(0);

pub static GLOBAL_DECAY: AtomicU8 = AtomicU8::new(0);
pub static PTRIM_DECAY: AtomicU8 = AtomicU8::new(0);

unsafe fn decide_global(decay: &TimeDecay) -> bool {
    if (OX_TRIM_THRESHOLD.load(Ordering::Relaxed) < decay.get_threshold() as usize)
        && (OX_TRIM_THRESHOLD.load(Ordering::Relaxed) != 0)
    {
        OX_TRIM_THRESHOLD.store(decay.get_threshold() as usize, Ordering::Relaxed);
    }

    let total = TOTAL_TIME_GLOBAL.load(Ordering::Relaxed);
    LAST_PRESSURE_CHECK.store(check_memory_pressure(), Ordering::Relaxed);

    if LAST_PRESSURE_CHECK.load(Ordering::Relaxed) > 85 {
        return true;
    }

    let timing = AVERAGE_BLOCK_TIMES_GLOBAL.load(Ordering::Relaxed);
    if timing == 0 {
        return false;
    }

    let last = LAST_TRIM_GLOBAL.load(Ordering::Relaxed);
    if total.saturating_sub(last) >= timing && total != 0 {
        LAST_TRIM_GLOBAL.store(total, Ordering::Relaxed);
        return true;
    }

    false
}

fn check_memory_pressure() -> usize {
    unsafe {
        let mut info: libc::sysinfo = std::mem::zeroed();

        if libc::sysinfo(&mut info) != 0 {
            return 50;
        }

        let unit = info.mem_unit as usize;
        let total_ram = (info.totalram as usize).saturating_mul(unit);
        let free_ram = (info.freeram as usize).saturating_mul(unit);
        let total_swap = (info.totalswap as usize).saturating_mul(unit);
        let free_swap = (info.freeswap as usize).saturating_mul(unit);

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
        loop {
            let decay = TimeDecay::from_u8(GLOBAL_DECAY.load(Ordering::Relaxed));
            std::thread::sleep(Duration::from_millis(decay.get_trim_time()));

            TOTAL_TIME_GLOBAL
                .fetch_add(decay.get_trim_time_for_global() as usize, Ordering::Relaxed);

            let time = get_clock().elapsed().as_secs() as u32;
            OX_CURRENT_STAMP = time;

            if decide_global(&decay) {
                GTrim.trim(OX_TRIM_THRESHOLD.load(Ordering::Relaxed));
            }
        }
    });
}
