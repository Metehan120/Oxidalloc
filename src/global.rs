use std::{
    hint::spin_loop,
    os::raw::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use crate::{OxHeader, free::is_ours};

pub static GLOBAL: [AtomicPtr<OxHeader>; 22] = [const { AtomicPtr::new(null_mut()) }; 22];
pub static GLOBAL_USAGE: [AtomicUsize; 22] = [const { AtomicUsize::new(0) }; 22];

pub struct GlobalHandler;

impl GlobalHandler {
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn push_to_global(&self, class: usize, head: *mut OxHeader, tail: *mut OxHeader) {
        loop {
            let current_head = GLOBAL[class].load(Ordering::Acquire);
            (*tail).next = current_head;

            if GLOBAL[class]
                .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return;
            }

            spin_loop();
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut OxHeader {
        loop {
            let current_head = GLOBAL[class].load(Ordering::Acquire);

            if current_head.is_null() {
                return null_mut();
            }

            let mut tail = current_head;
            let mut count = 1;
            while count < batch_size
                && !(*tail).next.is_null()
                && is_ours((*tail).next as *mut c_void)
            {
                tail = (*tail).next;
                count += 1;
            }

            let new_head = (*tail).next;

            if GLOBAL[class]
                .compare_exchange(current_head, new_head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                GLOBAL_USAGE[class].fetch_sub(count, Ordering::Relaxed);
                (*tail).next = null_mut();
                return current_head;
            }

            spin_loop();
        }
    }
}
