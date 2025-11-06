use std::{
    ptr::null_mut,
    sync::atomic::{AtomicPtr, Ordering},
};

use crate::Header;

// Global map list for shared memory allocation
static GLOBAL_MAP_LIST: [AtomicPtr<Header>; 20] = [
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
];

pub struct GlobalHandler;

impl GlobalHandler {
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn push_to_global(&self, class: usize, head: *mut Header, tail: *mut Header) {
        loop {
            let current_head = GLOBAL_MAP_LIST[class].load(Ordering::Acquire);
            (*tail).next = current_head;

            if GLOBAL_MAP_LIST[class]
                .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut Header {
        for _ in 0..1000 {
            let current_head = GLOBAL_MAP_LIST[class].load(Ordering::Acquire);

            if current_head.is_null() {
                return null_mut();
            }

            // Walk to find tail of batch
            let mut tail = current_head;
            let mut count = 1;
            while count < batch_size && !(*tail).next.is_null() {
                tail = (*tail).next;
                count += 1;
            }

            let new_head = (*tail).next;

            if GLOBAL_MAP_LIST[class]
                .compare_exchange(current_head, new_head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                (*tail).next = null_mut(); // Detach batch
                return current_head;
            }
        }

        null_mut()
    }
}
