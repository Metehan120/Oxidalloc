#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    OxHeader,
    slab::{NUM_SIZE_CLASSES, quarantine::quarantine},
    va::va_helper::is_ours,
};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::{hint::spin_loop, ptr::null_mut};

pub static GLOBAL: [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES] =
    [const { AtomicPtr::new(null_mut()) }; NUM_SIZE_CLASSES];
pub static GLOBAL_USAGE: [AtomicUsize; NUM_SIZE_CLASSES] =
    [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES];

pub struct GlobalHandler;

impl GlobalHandler {
    pub unsafe fn push_to_global(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        loop {
            let current_head = GLOBAL[class].load(Ordering::Acquire);
            (*tail).next = current_head;

            if GLOBAL[class]
                .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                GLOBAL_USAGE[class].fetch_add(batch_size, Ordering::Relaxed);
                return;
            }

            spin_loop();
        }
    }

    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut OxHeader {
        loop {
            let current_head = GLOBAL[class].load(Ordering::Acquire);

            if current_head.is_null() {
                return null_mut();
            }

            if !is_ours(current_head as usize) {
                // Quarantine the header
                quarantine(None, current_head as usize, class);
                if GLOBAL[class]
                    .compare_exchange(
                        current_head,
                        null_mut(),
                        Ordering::Release,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    GLOBAL_USAGE[class].store(0, Ordering::Relaxed);
                }
                return null_mut();
            }

            if current_head.is_null() {
                continue;
            }

            let mut tail = current_head;
            let mut count = 1;
            // Loop through the list until we reach the end or the batch size is reached
            while count < batch_size && !(*tail).next.is_null() && is_ours((*tail).next as usize) {
                tail = (*tail).next;
                count += 1;
            }

            let new_head = (*tail).next;

            if GLOBAL[class]
                .compare_exchange(current_head, new_head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                GLOBAL_USAGE[class].fetch_sub(count, Ordering::Relaxed);
                // Set next pointer to null to avoid inconsistency
                (*tail).next = null_mut();
                return current_head;
            }

            spin_loop();
        }
    }
}
