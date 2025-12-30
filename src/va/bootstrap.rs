#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Mutex, Once,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use libc::getrandom;
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous};

use crate::{
    OX_TRIM_THRESHOLD, OxidallocError,
    slab::thread_local::{THREAD_REGISTER, ThreadLocalEngine},
};

pub static VA_START: AtomicUsize = AtomicUsize::new(0);
pub static VA_END: AtomicUsize = AtomicUsize::new(0);
pub static VA_LEN: AtomicUsize = AtomicUsize::new(0);
pub static mut GLOBAL_RANDOM: usize = 0;

pub static IS_BOOTSTRAP: AtomicBool = AtomicBool::new(true);
pub static BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());

pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);
pub static ONCE: Once = Once::new();

extern "C" fn allocator_shutdown() {
    SHUTDOWN.store(true, Ordering::Release);
}

pub fn register_shutdown() {
    unsafe {
        libc::atexit(allocator_shutdown);
    }
}

fn init_random() {
    unsafe {
        let mut rand: usize = 0;
        let ret = getrandom(
            &mut rand as *mut usize as *mut c_void,
            size_of::<usize>(),
            0,
        );
        eprintln!("Rand: {}", rand);
        if ret as usize != size_of::<usize>() {
            OxidallocError::SecurityViolation.log_and_abort(
                null_mut(),
                "Failed to initialize random number generator",
                None,
            );
        }
        GLOBAL_RANDOM = rand;
    }
}

pub fn init_threshold() {
    unsafe {
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
        } else {
            OX_TRIM_THRESHOLD.store(1024 * 1024 * 10, Ordering::Relaxed);
        }
    }
}

pub unsafe fn boot_strap() {
    if !IS_BOOTSTRAP.load(Ordering::Relaxed) {
        return;
    }
    let _lock = match BOOTSTRAP_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !IS_BOOTSTRAP.load(Ordering::Relaxed) {
        return;
    }
    IS_BOOTSTRAP.store(false, Ordering::Relaxed);
    THREAD_REGISTER.store(null_mut(), Ordering::Relaxed);

    if IS_BOOTSTRAP.load(Ordering::Relaxed) {
        return;
    }
    ONCE.call_once(|| {
        SHUTDOWN.store(false, Ordering::Relaxed);
        ThreadLocalEngine::get_or_init();
        va_init();
        register_shutdown();
        init_threshold();
        init_random();
    });
}

pub unsafe fn va_init() {
    const MIN_RESERVE: usize = 1024 * 1024 * 1024 * 4;
    const MAX_SIZE: usize = 1024 * 1024 * 1024 * 256;
    let mut size = MAX_SIZE;

    if MAX_SIZE < MIN_RESERVE {
        size = MIN_RESERVE;
    }

    loop {
        let probe = mmap_anonymous(
            null_mut(),
            size,
            ProtFlags::empty(),
            MapFlags::PRIVATE | MapFlags::NORESERVE,
        );

        match probe {
            Ok(output) => {
                VA_START.store(output as usize, Ordering::Relaxed);
                VA_END.store((output as usize) + size, Ordering::Relaxed);
                VA_LEN.store(size, Ordering::Relaxed);
                return;
            }
            Err(err) => {
                if size <= MIN_RESERVE {
                    OxidallocError::VAIinitFailed.log_and_abort(
                        0 as *mut c_void,
                        "Init failed during BOOTSTRAP: No available VA reserve",
                        Some(err),
                    )
                }

                size /= 2;
            }
        }
    }
}
