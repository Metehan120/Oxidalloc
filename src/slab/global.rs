#![allow(unsafe_op_in_unsafe_fn)]

use libc::sched_getcpu;

use crate::{
    OxHeader,
    slab::{NUM_SIZE_CLASSES, quarantine::quarantine},
    va::va_helper::is_ours,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{hint::spin_loop, ptr::null_mut};

// NOTE: best-effort CPU sharding, not true NUMA topology
pub const MAX_NUMA_NODES: usize = 12;
pub unsafe fn current_numa_node() -> usize {
    (sched_getcpu() as usize) % MAX_NUMA_NODES
}
thread_local! {
    static NUMA_NODE: usize = unsafe { current_numa_node() };
}

pub static GLOBAL: [[AtomicUsize; NUM_SIZE_CLASSES]; MAX_NUMA_NODES] =
    [const { [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES] }; MAX_NUMA_NODES];
pub static GLOBAL_USAGE: [[AtomicUsize; NUM_SIZE_CLASSES]; MAX_NUMA_NODES] =
    [const { [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES] }; MAX_NUMA_NODES];
pub static GLOBAL_LOCKS: [[AtomicBool; NUM_SIZE_CLASSES]; MAX_NUMA_NODES] =
    [const { [const { AtomicBool::new(false) }; NUM_SIZE_CLASSES] }; MAX_NUMA_NODES];

pub struct GlobalHandler;

impl GlobalHandler {
    #[inline(always)]
    fn lock(&self, class: usize) {
        let node = NUMA_NODE.with(|node| *node);
        while GLOBAL_LOCKS[node][class]
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
    }

    #[inline(always)]
    fn unlock(&self, class: usize) {
        let node = NUMA_NODE.with(|node| *node);
        GLOBAL_LOCKS[node][class].store(false, Ordering::Release);
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
        let node = NUMA_NODE.with(|node| *node);
        let current_head = GLOBAL[node][class].load(Ordering::Relaxed) as *mut OxHeader;

        (*tail).next = current_head;
        GLOBAL[node][class].store(head as usize, Ordering::Relaxed);
        GLOBAL_USAGE[node][class].fetch_add(batch_size, Ordering::Relaxed);

        self.unlock(class);
    }

    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut OxHeader {
        self.lock(class);
        let node = NUMA_NODE.with(|node| *node);

        let current_head = GLOBAL[node][class].load(Ordering::Relaxed) as *mut OxHeader;
        if current_head.is_null() {
            self.unlock(class);
            return null_mut();
        }

        if !is_ours(current_head as usize) {
            // Quarantine the header
            quarantine(None, current_head as usize, class, false);
            GLOBAL[node][class].store(null_mut() as *mut OxHeader as usize, Ordering::Relaxed);
            GLOBAL_USAGE[node][class].store(0, Ordering::Relaxed);
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
        GLOBAL[node][class].store(new_head as usize, Ordering::Relaxed);
        GLOBAL_USAGE[node][class].fetch_sub(count, Ordering::Relaxed);

        self.unlock(class);

        current_head
    }
}
