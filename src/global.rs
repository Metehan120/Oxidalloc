use std::{
    hint::spin_loop,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

use crate::{OxHeader, free::is_ours}; // <--- Import is_ours

pub static GLOBAL: [AtomicPtr<OxHeader>; 22] = [const { AtomicPtr::new(null_mut()) }; 22];
pub static GLOBAL_USAGE: [AtomicUsize; 22] = [const { AtomicUsize::new(0) }; 22];
pub static GLOBAL_LOCKS: [AtomicBool; 22] = [const { AtomicBool::new(false) }; 22];

pub struct GlobalHandler;

impl GlobalHandler {
    #[inline(always)]
    fn lock(&self, class: usize) {
        while GLOBAL_LOCKS[class]
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
    }

    #[inline(always)]
    fn unlock(&self, class: usize) {
        GLOBAL_LOCKS[class].store(false, Ordering::Release);
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn push_to_global(&self, class: usize, head: *mut OxHeader, tail: *mut OxHeader) {
        self.lock(class);

        let current_head = GLOBAL[class].load(Ordering::Relaxed);

        (*tail).next = current_head;
        GLOBAL[class].store(head, Ordering::Relaxed);

        self.unlock(class);
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut OxHeader {
        self.lock(class);

        let current_head = GLOBAL[class].load(Ordering::Relaxed);

        if current_head.is_null() {
            self.unlock(class);
            return null_mut();
        }

        let mut tail = current_head;
        let mut count = 1;

        while count < batch_size {
            let next = (*tail).next;

            if next.is_null() {
                break;
            }

            if !is_ours(next as *mut std::ffi::c_void) {
                (*tail).next = null_mut();
                break;
            }

            tail = next;
            count += 1;
        }

        let new_head = (*tail).next;
        (*tail).next = null_mut();

        GLOBAL[class].store(new_head, Ordering::Relaxed);
        GLOBAL_USAGE[class].fetch_sub(count, Ordering::Relaxed);

        self.unlock(class);

        current_head
    }
}
