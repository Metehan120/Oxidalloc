#![allow(unsafe_op_in_unsafe_fn)]

use std::os::raw::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

use libc::{MAP_ANONYMOUS, MAP_FAILED, MAP_FIXED, MAP_PRIVATE, PROT_READ, PROT_WRITE, mmap};

use crate::internals::VA_MAP;

const BLOCK_SIZE: usize = 4096; // smallest block: 4KB
const MAX_ORDER: usize = 12; // max block = BLOCK_SIZE << MAX_ORDER
const ARENA_SIZE: usize = 32 * 1024 * 1024; // 32 MB

#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
}

static FREE_LISTS: [AtomicPtr<FreeNode>; MAX_ORDER + 1] =
    [const { AtomicPtr::new(null_mut()) }; MAX_ORDER + 1];

#[inline(always)]
fn order_to_size(order: usize) -> usize {
    BLOCK_SIZE << order
}

#[inline(always)]
fn buddy_of(base: usize, size: usize) -> usize {
    base ^ size
}

#[inline(always)]
unsafe fn push_free(order: usize, base: usize) {
    debug_assert!(order <= MAX_ORDER);
    debug_assert_eq!(base % BLOCK_SIZE, 0);

    let node = base as *mut FreeNode;
    let list = &FREE_LISTS[order];

    loop {
        let head = list.load(Ordering::Acquire);
        (*node).next = head;
        if list
            .compare_exchange(head, node, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            break;
        }
    }
}

#[inline(always)]
unsafe fn pop_free(order: usize) -> Option<usize> {
    debug_assert!(order <= MAX_ORDER);
    let list = &FREE_LISTS[order];

    loop {
        let head = list.load(Ordering::Acquire);
        if head.is_null() {
            return None;
        }
        let next = (*head).next;

        if list
            .compare_exchange(head, next, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return Some(head as usize);
        }
    }
}

unsafe fn add_arena() -> Option<()> {
    let hint = match VA_MAP.alloc(ARENA_SIZE) {
        Some(hint) => hint,
        None => return None,
    };

    let mem = mmap(
        hint as *mut c_void,
        ARENA_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
        -1,
        0,
    );

    if mem == MAP_FAILED {
        return None;
    }

    let base = mem as usize;
    let aligned = (base + (BLOCK_SIZE - 1)) & !(BLOCK_SIZE - 1);

    push_free(MAX_ORDER, aligned);

    Some(())
}

unsafe fn find_and_split(order: usize) -> Option<usize> {
    debug_assert!(order <= MAX_ORDER);

    for i in (order + 1)..=MAX_ORDER {
        if let Some(base) = pop_free(i) {
            let curr_base = base;
            let mut curr_order = i;
            while curr_order > order {
                let size = order_to_size(curr_order);
                let half = size / 2;
                let buddy = curr_base + half;
                push_free(curr_order - 1, buddy);
                curr_order -= 1;
            }
            return Some(curr_base);
        }
    }

    None
}

pub unsafe fn buddy_alloc(size: usize) -> Option<usize> {
    if size == 0 {
        return None;
    }

    // choose order
    let mut order = 0;
    while order_to_size(order) < size {
        order += 1;
        if order > MAX_ORDER {
            return None;
        }
    }

    // exact fit
    if let Some(base) = pop_free(order) {
        return Some(base);
    }

    // split larger block
    if let Some(base) = find_and_split(order) {
        return Some(base);
    }

    // need a fresh arena
    if add_arena().is_none() {
        return None;
    }

    // try again after arena
    if let Some(base) = pop_free(order) {
        return Some(base);
    }

    find_and_split(order)
}

pub unsafe fn buddy_free(base: usize, size: usize) {
    if size == 0 {
        return;
    }

    // figure out order from size
    let mut order = 0;
    while order_to_size(order) < size {
        order += 1;
        if order > MAX_ORDER {
            // size too big for this buddy config
            return;
        }
    }

    let mut block_size = order_to_size(order);
    let mut current_base = base;

    debug_assert_eq!(current_base % block_size, 0);

    // Try to coalesce upwards while the buddy is free and at the freelist head.
    'coalesce: loop {
        if order >= MAX_ORDER {
            break 'coalesce;
        }

        let buddy_addr = buddy_of(current_base, block_size);
        let list = &FREE_LISTS[order];

        // Try to steal the buddy only if it's exactly at the head.
        loop {
            let head = list.load(Ordering::Acquire);

            if head.is_null() {
                break 'coalesce;
            }

            if head as usize != buddy_addr {
                // buddy not free or not at head -> stop coalescing at this order
                break 'coalesce;
            }

            let next = (*head).next;

            // Try to unlink buddy from freelist
            if list
                .compare_exchange_weak(head, next, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // merge: new base is the lower of the two
                if buddy_addr < current_base {
                    current_base = buddy_addr;
                }

                order += 1;
                block_size <<= 1;
                debug_assert_eq!(current_base % block_size, 0);
                // continue outer coalesce loop with bigger block
                break;
            }

            // CAS failed, retry inner loop
        }
    }

    // push the (maybe-coalesced) block back to freelist
    push_free(order, current_base);
}
