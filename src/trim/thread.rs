use std::{
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    time::Duration,
};

use rustix::system::sysinfo;

use crate::{
    AVERAGE_BLOCK_TIMES_GLOBAL, OX_CURRENT_STAMP, OX_TRIM_THRESHOLD, get_clock,
    trim::{TimeDecay, gtrim::GTrim},
};

static TOTAL_TIME_GLOBAL: AtomicUsize = AtomicUsize::new(0);
static LAST_TRIM_GLOBAL: AtomicUsize = AtomicUsize::new(0);
pub static LAST_PRESSURE_CHECK: AtomicUsize = AtomicUsize::new(0);

pub static GLOBAL_DECAY: AtomicU8 = AtomicU8::new(0);

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
    let info = sysinfo();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inner::alloc::alloc_inner;
    use crate::inner::free::free_inner;
    use crate::internals::size_t;
    use crate::slab::interconnect::ICC;
    use crate::slab::{SIZE_CLASSES, TLS_MAX_BLOCKS, match_size_class};
    use crate::{AVERAGE_BLOCK_TIMES_GLOBAL, OX_CURRENT_STAMP};
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn trim_releases_large_class_blocks() {
        let _guard = TEST_LOCK.lock().unwrap();
        const SIZE: usize = 8192;
        let class = match_size_class(SIZE).expect("size class exists");
        assert!(SIZE_CLASSES[class] >= 4096, "test targets >= 4k classes");

        let count = 128;
        let mut ptrs = Vec::with_capacity(count);

        unsafe {
            OX_CURRENT_STAMP = 1;
        }

        for _ in 0..count {
            let ptr = unsafe { alloc_inner(SIZE as size_t) };
            assert!(!ptr.is_null());
            ptrs.push(ptr);
        }

        for ptr in ptrs {
            unsafe { free_inner(ptr) };
        }

        unsafe {
            OX_CURRENT_STAMP = 10_000;
        }
        AVERAGE_BLOCK_TIMES_GLOBAL.store(1, Ordering::Relaxed);

        let (ok, freed) = unsafe { GTrim.trim(0) };
        assert_eq!(ok, 1, "trim should report success");
        assert!(
            freed > 0,
            "trim should release some bytes for large classes"
        );
    }

    #[test]
    fn trim_releases_very_large_class_blocks() {
        let _guard = TEST_LOCK.lock().unwrap();
        const SIZE: usize = 65536;
        let class = match_size_class(SIZE).expect("size class exists");
        assert!(SIZE_CLASSES[class] >= 4096, "test targets >= 4k classes");

        let count = 64;
        let mut ptrs = Vec::with_capacity(count);

        unsafe {
            OX_CURRENT_STAMP = 1;
        }

        for _ in 0..count {
            let ptr = unsafe { alloc_inner(SIZE as size_t) };
            assert!(!ptr.is_null());
            ptrs.push(ptr);
        }

        for ptr in ptrs {
            unsafe { free_inner(ptr) };
        }

        unsafe {
            OX_CURRENT_STAMP = 10_000;
        }
        AVERAGE_BLOCK_TIMES_GLOBAL.store(1, Ordering::Relaxed);

        let (ok, freed) = unsafe { GTrim.trim(0) };
        assert_eq!(ok, 1, "trim should report success");
        assert!(
            freed > 0,
            "trim should release some bytes for large classes"
        );
    }

    #[test]
    fn trim_does_not_touch_sub_4k_icc_class() {
        let _guard = TEST_LOCK.lock().unwrap();
        const SIZE: usize = 1024;
        let class = match_size_class(SIZE).expect("size class exists");
        assert!(SIZE_CLASSES[class] < 4096, "test targets sub-4k classes");

        // Allocate and free enough blocks to force some into the ICC for this class.
        let count = TLS_MAX_BLOCKS[class] + 64;
        let mut ptrs = Vec::with_capacity(count);

        unsafe {
            OX_CURRENT_STAMP = 1;
        }

        for _ in 0..count {
            let ptr = unsafe { alloc_inner(SIZE as size_t) };
            assert!(!ptr.is_null());
            ptrs.push(ptr);
        }

        for ptr in ptrs {
            unsafe { free_inner(ptr) };
        }

        let before = unsafe { ICC.get_size(class) };
        assert!(before > 0, "expected sub-4k blocks to reach ICC");

        unsafe {
            OX_CURRENT_STAMP = 20_000;
        }
        AVERAGE_BLOCK_TIMES_GLOBAL.store(1, Ordering::Relaxed);

        let (_ok, _freed) = unsafe { GTrim.trim(0) };
        let after = unsafe { ICC.get_size(class) };
        assert_eq!(after, before, "trim should not touch sub-4k ICC class");
    }
}
