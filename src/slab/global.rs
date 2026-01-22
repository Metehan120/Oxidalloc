use crate::{
    MAX_NUMA_NODES, OxHeader, REAL_NUMA_NODES,
    slab::{NUM_SIZE_CLASSES, xor_ptr_numa},
};
#[cfg(feature = "hardened-linked-list")]
use std::hint::spin_loop;
use std::{
    hint::{likely, unlikely},
    sync::atomic::{AtomicUsize, Ordering},
};
use std::{ptr::null_mut, sync::atomic::AtomicBool};

const TAG_BITS: usize = 4;
const TAG_MASK: usize = (1 << TAG_BITS) - 1;
const PTR_MASK: usize = !TAG_MASK;

#[inline(always)]
fn pack(ptr: *mut OxHeader, tag: usize) -> usize {
    (ptr as usize) | (tag & TAG_MASK)
}

#[inline(always)]
fn unpack_ptr(val: usize) -> *mut OxHeader {
    (val & PTR_MASK) as *mut OxHeader
}

#[inline(always)]
fn unpack_tag(val: usize) -> usize {
    val & TAG_MASK
}

// ----------------------------------

#[repr(C, align(64))]
pub struct NumaGlobal {
    pub list: [AtomicUsize; NUM_SIZE_CLASSES],
    pub usage: [AtomicUsize; NUM_SIZE_CLASSES],
}

pub static GLOBAL_INIT: AtomicBool = AtomicBool::new(true);
pub static GLOBAL: [NumaGlobal; MAX_NUMA_NODES] = [const {
    NumaGlobal {
        list: [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES],
        usage: [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES],
    }
}; MAX_NUMA_NODES];

#[cfg(feature = "hardened-linked-list")]
static GLOBAL_LOCKS: [[AtomicBool; NUM_SIZE_CLASSES]; MAX_NUMA_NODES] =
    [const { [const { AtomicBool::new(false) }; NUM_SIZE_CLASSES] }; MAX_NUMA_NODES];

#[cfg(feature = "hardened-linked-list")]
struct GlobalLockGuard {
    lock: &'static AtomicBool,
}

#[cfg(feature = "hardened-linked-list")]
impl Drop for GlobalLockGuard {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}

#[cfg(feature = "hardened-linked-list")]
fn lock_global(numa_node_id: usize, class: usize) -> GlobalLockGuard {
    let lock = &GLOBAL_LOCKS[numa_node_id][class];
    while lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }
    GlobalLockGuard { lock }
}

pub struct GlobalHandler;

impl GlobalHandler {
    pub unsafe fn push_to_global(
        &self,
        class: usize,
        numa_node_id: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        #[cfg(feature = "hardened-linked-list")]
        let _guard = lock_global(numa_node_id, class);

        #[cfg(feature = "hardened-linked-list")]
        {
            use crate::slab::xor_ptr_numa;

            let mut curr = head;
            while curr != tail {
                let next_raw = (*curr).next;
                (*curr).next = xor_ptr_numa(next_raw, numa_node_id);
                curr = next_raw;
            }
        }

        loop {
            let cur = GLOBAL[numa_node_id].list[class].load(Ordering::Relaxed);
            let cur_ptr = unpack_ptr(cur);
            let cur_tag = unpack_tag(cur);

            (*tail).next = cur_ptr;

            let new = pack(
                xor_ptr_numa(head, numa_node_id) as *mut OxHeader,
                cur_tag.wrapping_add(1),
            );

            if GLOBAL[numa_node_id].list[class]
                .compare_exchange(cur, new, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                GLOBAL[numa_node_id].usage[class].fetch_add(batch_size, Ordering::Relaxed);
                return;
            }
        }
    }

    pub unsafe fn pop_from_global(
        &self,
        preferred_node: usize,
        class: usize,
        batch_size: usize,
    ) -> *mut OxHeader {
        let res = self.pop_from_shard(preferred_node, class, batch_size);
        if likely(!res.is_null()) {
            return res;
        }

        // Equals to 0 is just a safety check
        if REAL_NUMA_NODES == 1 || unlikely(REAL_NUMA_NODES == 0) {
            return null_mut();
        }

        // If resident is null (empty) then try to steal from other nodes
        //
        // # Safety
        // The caller must ensure `preffered_node` is a valid index within `MAX_NUMA_NODES`.
        // Returns `null_mut()` if no blocks are available in any NUMA shard.
        for i in 1..(REAL_NUMA_NODES % MAX_NUMA_NODES) {
            let neighbor = (preferred_node + i) % MAX_NUMA_NODES;
            let res = self.pop_from_shard(neighbor, class, batch_size);
            if !res.is_null() {
                return res;
            }
        }

        null_mut()
    }

    pub unsafe fn pop_from_global_local(
        &self,
        numa_node_id: usize,
        class: usize,
        batch_size: usize,
    ) -> *mut OxHeader {
        self.pop_from_shard(numa_node_id, class, batch_size)
    }

    unsafe fn pop_from_shard(
        &self,
        numa_node_id: usize,
        class: usize,
        batch_size: usize,
    ) -> *mut OxHeader {
        #[cfg(feature = "hardened-linked-list")]
        let _guard = lock_global(numa_node_id, class);

        loop {
            let cur = GLOBAL[numa_node_id].list[class].load(Ordering::Relaxed);
            let head_enc = unpack_ptr(cur);
            let tag = unpack_tag(cur);
            if unlikely(head_enc.is_null()) {
                return null_mut();
            }

            let head = xor_ptr_numa(head_enc, numa_node_id);

            let mut tail = head;
            let mut count = 1;
            for _ in 1..batch_size {
                let next_enc = (*tail).next;
                if unlikely(next_enc.is_null()) {
                    break;
                }
                tail = xor_ptr_numa(next_enc, numa_node_id);
                count += 1;
            }

            let new_head_enc = (*tail).next;
            let new = pack(new_head_enc, tag.wrapping_add(1));

            if GLOBAL[numa_node_id].list[class]
                .compare_exchange(cur, new, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                GLOBAL[numa_node_id].usage[class].fetch_sub(count, Ordering::Relaxed);

                #[cfg(feature = "hardened-linked-list")]
                {
                    let mut curr = head;
                    while curr != tail {
                        let next_enc = (*curr).next;
                        let next_raw = xor_ptr_numa(next_enc, numa_node_id);
                        (*curr).next = next_raw;
                        curr = next_raw;
                    }
                }
                (*tail).next = null_mut();

                return head;
            }
        }
    }
}
