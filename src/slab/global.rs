#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    OxHeader,
    slab::{NUM_SIZE_CLASSES, quarantine::quarantine},
    va::va_helper::is_ours,
};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

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

pub static GLOBAL: [AtomicUsize; NUM_SIZE_CLASSES] =
    [const { AtomicUsize::new(0) }; NUM_SIZE_CLASSES];
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
            let cur = GLOBAL[class].load(Ordering::Relaxed);
            let cur_ptr = unpack_ptr(cur);
            let cur_tag = unpack_tag(cur);

            (*tail).next = cur_ptr;

            let new = pack(head, cur_tag.wrapping_add(1));

            if GLOBAL[class]
                .compare_exchange(cur, new, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                GLOBAL_USAGE[class].fetch_add(batch_size, Ordering::Relaxed);
                return;
            }
        }
    }

    pub unsafe fn pop_batch_from_global(&self, class: usize, batch_size: usize) -> *mut OxHeader {
        loop {
            let cur = GLOBAL[class].load(Ordering::Relaxed);
            let head = unpack_ptr(cur);
            let tag = unpack_tag(cur);

            if head.is_null() {
                return null_mut();
            }

            if !is_ours(head as usize) {
                quarantine(None, head as usize, class);
                GLOBAL[class].store(0, Ordering::Relaxed);
                GLOBAL_USAGE[class].store(0, Ordering::Relaxed);
                return null_mut();
            }

            let mut tail = head;
            let mut count = 1;
            while count < batch_size && !(*tail).next.is_null() {
                tail = (*tail).next;
                count += 1;
            }

            let new_head = (*tail).next;
            let new = pack(new_head, tag.wrapping_add(1));

            if GLOBAL[class]
                .compare_exchange(cur, new, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                (*tail).next = null_mut();
                GLOBAL_USAGE[class].fetch_sub(count, Ordering::Relaxed);
                return head;
            }
        }
    }
}
