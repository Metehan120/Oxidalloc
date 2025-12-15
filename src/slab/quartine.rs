use std::{
    ptr::null_mut,
    sync::{Mutex, OnceLock},
};

use crate::OxidallocError;

#[cfg(debug_assertions)]
pub static MAX_QUARANTINE: usize = 1024 * 1024;
#[cfg(not(debug_assertions))]
pub static MAX_QUARANTINE: usize = 1024 * 64;

pub static QUARANTINE: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

pub fn quarantine(ptr: usize) {
    let q = QUARANTINE.get_or_init(|| Mutex::new(Vec::with_capacity(MAX_QUARANTINE)));
    let mut guard = match q.lock() {
        Ok(quartine) => quartine,
        Err(_) => return, // we can ignore it
    };

    if guard.contains(&ptr) {
        OxidallocError::DoubleQuarantine.log_and_abort(
            null_mut(),
            "Double quarantine detected, aborting process",
            None,
        )
    }

    if guard.len() < MAX_QUARANTINE {
        guard.push(ptr);
    } else {
        OxidallocError::TooMuchQuarantine.log_and_abort(
            null_mut(),
            "Too much quarantine detected, aborting process",
            None,
        )
    }
}
