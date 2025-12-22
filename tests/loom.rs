#![allow(unsafe_op_in_unsafe_fn)]

use loom::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use loom::thread;
use std::sync::Once;
use std::{hint::spin_loop, ptr::null_mut};

use oxidalloc::{
    OxHeader,
    slab::{NUM_SIZE_CLASSES, quarantine::quarantine},
    va::va_helper::is_ours,
};

//
// ===== GLOBAL STATE (LOOM ONLY) =====
//

static INIT: Once = Once::new();

static mut GLOBAL: *const [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES] = std::ptr::null();
static mut GLOBAL_USAGE: *const [AtomicUsize; NUM_SIZE_CLASSES] = std::ptr::null();

fn init_globals() {
    INIT.call_once(|| unsafe {
        let mut g: Vec<AtomicPtr<OxHeader>> = Vec::with_capacity(NUM_SIZE_CLASSES);
        let mut u: Vec<AtomicUsize> = Vec::with_capacity(NUM_SIZE_CLASSES);

        for _ in 0..NUM_SIZE_CLASSES {
            g.push(AtomicPtr::new(null_mut()));
            u.push(AtomicUsize::new(0));
        }

        GLOBAL = Box::into_raw(Box::new(g.try_into().unwrap()));
        GLOBAL_USAGE = Box::into_raw(Box::new(u.try_into().unwrap()));
    });
}

#[inline(always)]
fn global(class: usize) -> &'static AtomicPtr<OxHeader> {
    init_globals();
    unsafe { &(*GLOBAL)[class] }
}

#[inline(always)]
fn global_usage(class: usize) -> &'static AtomicUsize {
    init_globals();
    unsafe { &(*GLOBAL_USAGE)[class] }
}

//
// ===== HANDLER =====
//

pub struct GlobalHandler;

impl GlobalHandler {
    pub unsafe fn push_to_global(
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        let global = global(class);
        let usage = global_usage(class);

        loop {
            let cur = global.load(Ordering::Acquire);
            (*tail).next = cur;

            if global
                .compare_exchange(cur, head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                usage.fetch_add(batch_size, Ordering::Relaxed);
                return;
            }

            spin_loop();
        }
    }

    pub unsafe fn pop_batch_from_global(class: usize, batch_size: usize) -> *mut OxHeader {
        let global = global(class);
        let usage = global_usage(class);

        loop {
            let head = global.load(Ordering::Acquire);
            if head.is_null() {
                return null_mut();
            }

            if !is_ours(head as usize) {
                quarantine(None, head as usize, class);
                if global
                    .compare_exchange(head, null_mut(), Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    usage.store(0, Ordering::Relaxed);
                }
                return null_mut();
            }

            let mut tail = head;
            let mut count = 1;

            while count < batch_size && !(*tail).next.is_null() && is_ours((*tail).next as usize) {
                tail = (*tail).next;
                count += 1;
            }

            let new_head = (*tail).next;

            if global
                .compare_exchange(head, new_head, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                usage.fetch_sub(count, Ordering::Relaxed);
                (*tail).next = null_mut();
                return head;
            }

            spin_loop();
        }
    }
}

//
// ===== LOOM TEST =====
//

fn make_node() -> *mut OxHeader {
    use oxidalloc::MAGIC;
    Box::leak(Box::new(OxHeader {
        next: null_mut(),
        magic: MAGIC,
        in_use: 0,
        life_time: 0,
        size: 0,
        flag: 0,
    }))
}

#[test]
fn loom_global_usage_race() {
    loom::model(|| {
        let a = make_node();
        let b = make_node();

        let t1 = thread::spawn(move || unsafe {
            GlobalHandler::push_to_global(0, a, a, 1);
        });

        let t2 = thread::spawn(move || unsafe {
            let _ = GlobalHandler::pop_batch_from_global(0, 1);
        });

        let t3 = thread::spawn(move || unsafe {
            GlobalHandler::push_to_global(0, b, b, 1);
        });

        let t4 = thread::spawn(move || unsafe {
            let _ = GlobalHandler::pop_batch_from_global(0, 1);
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();
        t4.join().unwrap();

        let u = global_usage(0).load(Ordering::Relaxed);
        assert!(u == 0 || u == 1);
    });
}
