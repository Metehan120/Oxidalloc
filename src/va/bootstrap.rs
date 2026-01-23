use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Mutex, Once,
        atomic::{AtomicBool, Ordering},
    },
};

use libc::getrandom;
#[cfg(feature = "hardened-linked-list")]
use rustix::mm::{Advice, madvise};

use crate::{
    FREED_MAGIC, MAGIC, MAX_NUMA_NODES, OX_MAX_RESERVATION, OX_TRIM_THRESHOLD, OX_USE_THP,
    OxidallocError, REAL_NUMA_NODES, slab::thread_local::ThreadLocalEngine, va::bitmap::ALLOC_RNG,
};

pub static IS_BOOTSTRAP: AtomicBool = AtomicBool::new(true);
pub static BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());
pub static mut GLOBAL_RANDOM: usize = 0;
pub static mut PER_NUMA_KEY: [usize; MAX_NUMA_NODES] = [0; MAX_NUMA_NODES];

pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn allocator_shutdown() {
    SHUTDOWN.store(true, Ordering::Release);
}

pub unsafe fn register_shutdown() {
    libc::atexit(allocator_shutdown);
}

fn detect_numa_nodes() -> usize {
    use libc::{SYS_get_mempolicy, c_int, c_ulong, syscall};

    const MPOL_F_MEMS_ALLOWED: c_ulong = 1 << 2;
    const MAX_NODES: usize = 1024;
    const BITS_PER_LONG: usize = std::mem::size_of::<c_ulong>() * 8;
    let mut mask: [c_ulong; 16] = [0; MAX_NODES / BITS_PER_LONG];

    let ret = unsafe {
        syscall(
            SYS_get_mempolicy,
            std::ptr::null_mut::<c_int>(),
            mask.as_mut_ptr(),
            MAX_NODES as c_ulong,
            0 as c_ulong,
            MPOL_F_MEMS_ALLOWED,
        )
    };

    if ret != 0 {
        return 1;
    }

    for (i, &word) in mask.iter().enumerate().rev() {
        if word != 0 {
            let bit_in_word = BITS_PER_LONG - 1 - (word.leading_zeros() as usize);
            return (i * BITS_PER_LONG) + bit_in_word;
        }
    }

    1
}

pub(crate) unsafe fn init_numa_nodes() {
    let nodes = detect_numa_nodes();
    REAL_NUMA_NODES = nodes;
}

pub(crate) unsafe fn init_magic() {
    let mut rand: [u64; 2] = [0; 2];
    let ret = getrandom(rand.as_mut_ptr() as *mut c_void, size_of::<u64>() * 2, 0);
    if ret.cast_unsigned() != (size_of::<u64>() * 2) {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }
    MAGIC = rand[0];
    FREED_MAGIC = rand[1];
}

#[cfg(feature = "hardened-linked-list")]
pub(crate) unsafe fn init_random_numa() {
    unsafe {
        let mut rand: [usize; MAX_NUMA_NODES] = [0; MAX_NUMA_NODES];
        let ret = getrandom(
            rand.as_mut_ptr() as *mut c_void,
            size_of::<usize>() * MAX_NUMA_NODES,
            0,
        );
        if ret.cast_unsigned() != (size_of::<usize>() * MAX_NUMA_NODES) {
            OxidallocError::SecurityViolation.log_and_abort(
                null_mut(),
                "Failed to initialize random number generator",
                None,
            );
        }

        // Keep low bits clear so XOR'd pointers remain aligned for tag packing.
        const TAG_MASK: usize = 0xF;
        for key in rand.iter_mut() {
            *key &= !TAG_MASK;
        }

        let _ = madvise(
            PER_NUMA_KEY.as_ptr() as *mut c_void,
            size_of::<usize>() * MAX_NUMA_NODES,
            Advice::LinuxDontDump,
        );

        PER_NUMA_KEY = rand;
    }
}

unsafe fn init_random() {
    let mut rand: usize = 0;
    let ret = getrandom(
        &mut rand as *mut usize as *mut c_void,
        size_of::<usize>(),
        0,
    );
    if ret.cast_unsigned() != size_of::<usize>() {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }
    GLOBAL_RANDOM = rand;
}

unsafe fn init_alloc_random() {
    let mut rand: u64 = 0;
    let ret = getrandom(&mut rand as *mut u64 as *mut c_void, size_of::<u64>(), 0);
    if ret.cast_unsigned() != size_of::<u64>() {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }
    ALLOC_RNG.store(rand, Ordering::Relaxed);
}

pub unsafe fn init_thp() {
    let key = b"OX_USE_THP\0";
    let value_ptr = libc::getenv(key.as_ptr() as *const i8);

    if !value_ptr.is_null() {
        let mut val = 0usize;
        let mut ptr = value_ptr as *const u8;

        while *ptr != 0 {
            if *ptr >= b'0' && *ptr <= b'9' {
                val = val * 10 + (*ptr - b'0') as usize;
            } else {
                break;
            }
            ptr = ptr.add(1);
        }

        if val == 1 {
            OX_USE_THP.store(true, Ordering::Relaxed);
        }
    }
}

pub unsafe fn init_threshold() {
    let key = b"OX_TRIM_THRESHOLD\0";
    let value_ptr = libc::getenv(key.as_ptr() as *const i8);

    if !value_ptr.is_null() {
        let mut val = 0usize;
        let mut ptr = value_ptr as *const u8;

        while *ptr != 0 {
            if *ptr >= b'0' && *ptr <= b'9' {
                val = val * 10 + (*ptr - b'0') as usize;
            } else {
                break;
            }
            ptr = ptr.add(1);
        }

        if val == 0 || val < 1024 * 1024 {
            val = 1024 * 1024;
        }
        OX_TRIM_THRESHOLD.store(val, Ordering::Relaxed);
    }
}

pub unsafe fn init_reverse() {
    let key = b"OX_MAX_RESERVATION\0";
    let value_ptr = libc::getenv(key.as_ptr() as *const i8);

    if !value_ptr.is_null() {
        let mut val = 0usize;
        let mut ptr = value_ptr as *const u8;

        while *ptr != 0 {
            if *ptr >= b'0' && *ptr <= b'9' {
                val = val * 10 + (*ptr - b'0') as usize;
            } else {
                break;
            }
            ptr = ptr.wrapping_add(1);
        }

        let next_power_of_two = val
            .checked_next_power_of_two()
            .unwrap_or(1024 * 1024 * 1024 * 16)
            .max(1024 * 1024 * 1024 * 16)
            .min(1024 * 1024 * 1024 * 1024 * 256);

        OX_MAX_RESERVATION.store(next_power_of_two, Ordering::Relaxed);
    }
}

static ONCE: Once = Once::new();

#[inline(always)]
pub unsafe fn boot_strap() {
    if !IS_BOOTSTRAP.load(Ordering::Relaxed) {
        return;
    }
    let _lock = match BOOTSTRAP_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !IS_BOOTSTRAP.load(Ordering::Acquire) {
        return;
    }
    IS_BOOTSTRAP.store(false, Ordering::Release);
    if IS_BOOTSTRAP.load(Ordering::Relaxed) {
        return;
    }
    ONCE.call_once(|| {
        SHUTDOWN.store(false, Ordering::Relaxed);
        ThreadLocalEngine::get_or_init();
        register_shutdown();
        init_reverse();
        init_threshold();
        init_thp();
        init_random();
        init_magic();
        init_alloc_random();
        init_numa_nodes();
        #[cfg(feature = "hardened-linked-list")]
        init_random_numa();
    });
}
