use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous};

use crate::{OxidallocError, slab::thread_local::ThreadLocalEngine};

pub static VA_START: AtomicUsize = AtomicUsize::new(0);
pub static VA_END: AtomicUsize = AtomicUsize::new(0);
pub static VA_LEN: AtomicUsize = AtomicUsize::new(0);

pub static IS_BOOTSTRAP: AtomicBool = AtomicBool::new(true);
pub static BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());

pub fn boot_strap() {
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

    va_init();
    ThreadLocalEngine::get_or_init();
    IS_BOOTSTRAP.store(false, Ordering::Relaxed);
}

pub fn va_init() {
    const MIN_RESERVE: usize = 1024 * 1024 * 1024 * 64;
    const MAX_SIZE: usize = 1024 * 1024 * 1024 * 256;
    let mut size = MAX_SIZE;

    if MAX_SIZE < MIN_RESERVE {
        size = MIN_RESERVE;
    }

    loop {
        unsafe {
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
}
