use std::{
    hint::{likely, spin_loop},
    sync::atomic::{AtomicU8, Ordering},
};

pub struct Once {
    state: AtomicU8,
}

// Use Spinlock-based Once implementation for better Fork Safety
impl Once {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
        }
    }

    pub fn call_once<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        if likely(self.state.load(Ordering::Relaxed) == 2) {
            return;
        }

        if self
            .state
            .compare_exchange(
                0,
                1,
                std::sync::atomic::Ordering::Acquire,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            f();
            self.state.store(2, std::sync::atomic::Ordering::Release);
        }

        while self.state.load(std::sync::atomic::Ordering::Acquire) != 2 {
            spin_loop();
        }
    }

    pub fn reset_at_fork(&self) {
        let _ = self
            .state
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed);
    }

    pub fn get_state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }
}
