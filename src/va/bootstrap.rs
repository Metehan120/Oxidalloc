#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    ptr::null_mut,
    sync::{
        Mutex, Once,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    OX_ENABLE_EXPERIMENTAL_HEALING, OX_MAX_RESERVATION, OX_TRIM_THRESHOLD, OX_USE_THP,
    slab::thread_local::{THREAD_REGISTER, ThreadLocalEngine},
};

pub static IS_BOOTSTRAP: AtomicBool = AtomicBool::new(true);
pub static BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());

pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn allocator_shutdown() {
    SHUTDOWN.store(true, Ordering::Release);
}

pub unsafe fn register_shutdown() {
    libc::atexit(allocator_shutdown);
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

pub unsafe fn init_healing() {
    let key = b"OX_ENABLE_EXPERIMENTAL_HEALING\0";
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
            OX_ENABLE_EXPERIMENTAL_HEALING.store(true, Ordering::Relaxed);
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
            ptr = ptr.add(1);
        }

        if val == 0 || val < 1024 * 1024 * 256 {
            val = 1024 * 1024 * 256;
        }

        OX_MAX_RESERVATION.store(val.next_power_of_two(), Ordering::Relaxed);
    }
}

static ONCE: Once = Once::new();

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
    THREAD_REGISTER.store(null_mut(), Ordering::Relaxed);
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
        init_healing();
    });
}
