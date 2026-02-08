#[cfg(not(feature = "global-alloc"))]
use std::sync::{Mutex, MutexGuard};
use std::{ptr::null_mut, sync::atomic::Ordering};

use crate::{
    FREED_MAGIC, MAGIC, OX_DISABLE_THP, OX_FORCE_THP, OX_MAX_RESERVATION, OX_TRIM,
    OX_TRIM_THRESHOLD, OxidallocError,
    internals::{env::get_env_usize, once::Once},
    slab::thread_local::ThreadLocalEngine,
    sys::memory_system::{get_cpu_count, getrandom},
};

pub static mut NTHREADS: usize = 0;
#[cfg(not(feature = "global-alloc"))]
pub static BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());
pub static mut GLOBAL_RANDOM: usize = 0;
pub static mut NUMA_KEY: usize = 0;

#[cfg(not(feature = "global-alloc"))]
static mut ATFORK_GUARD: Option<MutexGuard<'static, ()>> = None;

#[cfg(not(feature = "global-alloc"))]
extern "C" fn fork_prepare() {
    let guard = BOOTSTRAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        // Keep the lock held across fork so parent/child can safely drop it.

        use std::mem::transmute;
        ATFORK_GUARD = Some(transmute::<MutexGuard<'_, ()>, MutexGuard<'static, ()>>(
            guard,
        ));
    }
}

#[cfg(not(feature = "global-alloc"))]
extern "C" fn fork_parent() {
    unsafe {
        if let Some(guard) = ATFORK_GUARD.take() {
            drop(guard);
        }
    }
}

#[cfg(not(feature = "global-alloc"))]
extern "C" fn fork_child() {
    use crate::{
        inner::{alloc::reset_fork_thread_state, fallback::fallback_reinit_on_fork},
        va::bitmap::{reset_fork_locks, reset_fork_onces},
    };

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

#[cfg(not(feature = "global-alloc"))]
pub unsafe fn register_fork_handlers() {
    use crate::internals::pthread_atfork;

    let _ = pthread_atfork(Some(fork_prepare), Some(fork_parent), Some(fork_child));
}

unsafe fn init_nthreads() {
    let thread_num = get_cpu_count();
    NTHREADS = thread_num;
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

pub unsafe fn init_disable_thp() {
    let key = b"OX_DISABLE_THP";

    if let Some(val) = get_env_usize(key) {
        if val == 1 && !OX_FORCE_THP {
            OX_DISABLE_THP = true;
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

pub unsafe fn init_thread() {
    let key = b"OX_DISABLE_TRIM_THREAD";

    if let Some(val) = get_env_usize(key) {
        if val == 1 {
            OX_TRIM = false;
        }
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
        #[cfg(not(feature = "global-alloc"))]
        register_fork_handlers();
        init_reverse();
        init_threshold();
        init_thp();
        init_disable_thp();
        init_random();
        init_magic();
        init_thread();
        #[cfg(feature = "hardened-linked-list")]
        init_random_numa();
        init_nthreads();
    });
}
