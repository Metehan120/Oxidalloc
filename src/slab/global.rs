#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    OxHeader,
    slab::{NUM_SIZE_CLASSES, quarantine::quarantine},
    va::va_helper::is_ours,
};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::{hint::spin_loop, ptr::null_mut};

pub static GLOBAL: [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES] =
    [const { AtomicPtr::new(null_mut()) }; NUM_SIZE_CLASSES];
pub static GLOBAL_USAGE: [AtomicUsize; NUM_SIZE_CLASSES] =
    [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES];
pub static GLOBAL_LOCKS: [AtomicBool; NUM_SIZE_CLASSES] =
    [const { AtomicBool::new(false) }; NUM_SIZE_CLASSES];

pub struct GlobalHandler;

impl GlobalHandler {
    #[inline(always)]
    fn lock(&self, class: usize) {
        while GLOBAL_LOCKS[class]
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
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
    pub unsafe fn push_to_global(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        self.lock(class);

        let current_head = GLOBAL[class].load(Ordering::Relaxed);

        (*tail).next = current_head;
        GLOBAL[class].store(head, Ordering::Relaxed);
        GLOBAL_USAGE[class].fetch_add(batch_size, Ordering::Relaxed);

        self.unlock(class);
    }

    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut OxHeader {
        self.lock(class);

        let current_head = GLOBAL[class].load(Ordering::Relaxed);

        if current_head.is_null() {
            self.unlock(class);
            return null_mut();
        }

        if !is_ours(current_head as usize) {
            // Quarantine the header
            quarantine(None, current_head as usize, class, false);
            GLOBAL[class].store(null_mut(), Ordering::Relaxed);
            GLOBAL_USAGE[class].store(0, Ordering::Relaxed);
            self.unlock(class);
            return null_mut();
        }

        let mut tail = current_head;
        let mut count = 1;
        // Loop through the list until we reach the end or the batch size is reached
        while count < batch_size && !(*tail).next.is_null() && is_ours((*tail).next as usize) {
            tail = (*tail).next;
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
