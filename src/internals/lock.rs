use std::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "hardened-linked-list")]
use crate::slab::NUM_SIZE_CLASSES;

pub struct _LockGuard(*const AtomicBool);

impl Drop for _LockGuard {
    fn drop(&mut self) {
        unsafe {
            let lock = self.0;
            (*lock).store(false, Ordering::Relaxed);
        };
    }
}

pub struct SerialLock {
    state: AtomicBool,
}

impl SerialLock {
    pub const fn new() -> Self {
        SerialLock {
            state: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    pub fn lock(&self) -> _LockGuard {
        while self
            .state
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }

        let guard = _LockGuard(&self.state as *const AtomicBool);
        guard
    }

    #[inline(always)]
    pub fn unlock(&self) {
        self.state.store(false, Ordering::Release);
    }

    #[cfg(not(feature = "global-alloc"))]
    pub fn reset_on_fork(&self) {
        self.unlock();
    }
}

#[cfg(feature = "hardened-linked-list")]
pub struct GlobalLock {
    locks: [SerialLock; NUM_SIZE_CLASSES],
}

#[cfg(feature = "hardened-linked-list")]
impl GlobalLock {
    pub const fn new() -> Self {
        GlobalLock {
            locks: [const { SerialLock::new() }; NUM_SIZE_CLASSES],
        }
    }

    #[inline(always)]
    pub fn lock(&self, class: usize) -> _LockGuard {
        self.locks[class].lock()
    }

    #[inline(always)]
    pub fn unlock(&self, class: usize) {
        self.locks[class].unlock();
    }

    pub fn reset_on_fork(&self) {
        for class in 0..NUM_SIZE_CLASSES {
            self.locks[class].reset_on_fork();
        }
    }
}
