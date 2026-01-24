use std::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "hardened-linked-list")]
use crate::{MAX_NUMA_NODES, slab::NUM_SIZE_CLASSES};

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
    pub state: AtomicBool,
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

    pub fn reset_on_fork(&self) {
        self.unlock();
    }
}

#[cfg(feature = "hardened-linked-list")]
pub struct GlobalLock {
    locks: [[SerialLock; NUM_SIZE_CLASSES]; MAX_NUMA_NODES],
}

#[cfg(feature = "hardened-linked-list")]
impl GlobalLock {
    pub const fn new() -> Self {
        GlobalLock {
            locks: [const { [const { SerialLock::new() }; NUM_SIZE_CLASSES] }; MAX_NUMA_NODES],
        }
    }

    #[inline(always)]
    pub fn lock(&self, numa_node_id: usize, class: usize) -> _LockGuard {
        self.locks[numa_node_id][class].lock()
    }

    #[inline(always)]
    pub fn unlock(&self, numa_node_id: usize, class: usize) {
        self.locks[numa_node_id][class].unlock();
    }

    pub fn reset_on_fork(&self) {
        for node in 0..MAX_NUMA_NODES {
            for class in 0..NUM_SIZE_CLASSES {
                self.locks[node][class].reset_on_fork();
            }
        }
    }
}
