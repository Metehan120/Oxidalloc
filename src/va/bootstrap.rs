use std::{
    mem::transmute,
    ptr::null_mut,
    sync::{Mutex, MutexGuard, atomic::Ordering},
};

use crate::{
    FREED_MAGIC, MAGIC, OX_FORCE_THP, OX_MAX_RESERVATION, OX_TRIM_THRESHOLD, OxidallocError,
    REAL_NUMA_NODES,
    abi::{fallback::fallback_reinit_on_fork, malloc::reset_fork_thread_state},
    internals::{env::get_env_usize, once::Once},
    slab::thread_local::ThreadLocalEngine,
    sys::memory_system::{get_cpu_count, getrandom},
    va::bitmap::{reset_fork_locks, reset_fork_onces},
};

pub static mut NTHREADS: usize = 0;
pub static BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());
pub static mut GLOBAL_RANDOM: usize = 0;
pub static mut NUMA_KEY: usize = 0;

static mut ATFORK_GUARD: Option<MutexGuard<'static, ()>> = None;

extern "C" fn fork_prepare() {
    let guard = BOOTSTRAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        // Keep the lock held across fork so parent/child can safely drop it.
        ATFORK_GUARD = Some(transmute::<MutexGuard<'_, ()>, MutexGuard<'static, ()>>(
            guard,
        ));
    }
}

extern "C" fn fork_parent() {
    unsafe {
        if let Some(guard) = ATFORK_GUARD.take() {
            drop(guard);
        }
    }
}

extern "C" fn fork_child() {
    unsafe {
        if let Some(guard) = ATFORK_GUARD.take() {
            drop(guard);
        }
    }
    reset_fork_locks();
    reset_fork_onces();
    #[cfg(feature = "hardened-linked-list")]
    unsafe {
        crate::slab::global::reset_global_locks()
    };
    reset_fork_thread_state();
    crate::slab::reset_fork_onces();
    crate::reset_fork_onces();
    fallback_reinit_on_fork();
    ONCE.reset_at_fork();
}

pub unsafe fn register_fork_handlers() {
    let _ = libc::pthread_atfork(Some(fork_prepare), Some(fork_parent), Some(fork_child));
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
            return ((i * BITS_PER_LONG) + bit_in_word + 1).max(1);
        }
    }

    1
}

unsafe fn init_nthreads() {
    let thread_num = get_cpu_count();
    NTHREADS = thread_num;
}

pub(crate) unsafe fn init_numa_nodes() {
    let nodes = detect_numa_nodes();
    REAL_NUMA_NODES = nodes;
}

pub(crate) unsafe fn init_magic() {
    #[cfg(feature = "hardened-malloc")]
    let mut rand: [u64; 2] = [0; 2];
    #[cfg(not(feature = "hardened-malloc"))]
    let mut rand: [u8; 2] = [0; 2];

    let ret = getrandom(&mut rand);
    if ret.is_err() {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }

    if FREED_MAGIC == MAGIC {
        rand[1] = rand[1].saturating_sub(1);
    }

    MAGIC = rand[0];
    FREED_MAGIC = rand[1];
}

#[cfg(feature = "hardened-linked-list")]
pub(crate) unsafe fn init_random_numa() {
    unsafe {
        let mut rand: [usize; 1] = [0; 1];
        let ret = getrandom(&mut rand);
        if ret.is_err() {
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

        NUMA_KEY = rand[0];
    }
}

unsafe fn init_random() {
    let mut rand = [0u8; 8];

    let ret = getrandom(&mut rand);
    if ret.is_err() {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }
    GLOBAL_RANDOM = usize::from_ne_bytes(rand);
}

pub(crate) unsafe fn init_alloc_random() -> usize {
    let mut rand = [0u8; 8];

    let ret = getrandom(&mut rand);
    if ret.is_err() {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }
    usize::from_ne_bytes(rand)
}

pub unsafe fn init_thp() {
    let key = b"OX_FORCE_THP";

    if let Some(val) = get_env_usize(key) {
        if val == 1 {
            OX_FORCE_THP = true;
        }
    }
}

pub unsafe fn init_threshold() {
    let key = b"OX_TRIM_THRESHOLD";

    if let Some(mut val) = get_env_usize(key) {
        if val == 0 || val < 1024 * 1024 {
            val = 1024 * 1024;
        }
        OX_TRIM_THRESHOLD.store(val, Ordering::Relaxed);
    }
}

pub unsafe fn init_reverse() {
    let key = b"OX_MAX_RESERVATION";

    if let Some(val) = get_env_usize(key) {
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
    ONCE.call_once(|| {
        ThreadLocalEngine::get_or_init();
        register_fork_handlers();
        init_reverse();
        init_threshold();
        init_thp();
        init_random();
        init_magic();
        init_numa_nodes();
        #[cfg(feature = "hardened-linked-list")]
        init_random_numa();
        init_nthreads();
    });
}
