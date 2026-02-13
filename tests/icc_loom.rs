#![allow(unsafe_op_in_unsafe_fn)]
#![feature(likely_unlikely)]

use loom::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use loom::thread;
use std::hint::unlikely;
use std::ptr::null_mut;

const NUM_SIZE_CLASSES: usize = 34;
const NUMA_KEY: usize = 0x12345678;

#[repr(C, align(16))]
pub struct OxHeader {
    pub next: *mut OxHeader,
    pub class: u8,
    pub magic: u8,
    pub life_time: u32,
}

#[inline(always)]
pub unsafe fn xor_ptr_general(ptr: *mut OxHeader, _key: usize) -> *mut OxHeader {
    ptr
}

const ABA_TAG_BITS: usize = 4;
const ABA_TAG_MASK: usize = (1 << ABA_TAG_BITS) - 1;
const ABA_PTR_MASK: usize = !ABA_TAG_MASK;

#[inline(always)]
fn head_pack(ptr: *mut OxHeader, tag: usize) -> *mut OxHeader {
    ((ptr as usize) | (tag & ABA_TAG_MASK)) as *mut OxHeader
}

#[inline(always)]
fn head_ptr(val: *mut OxHeader) -> *mut OxHeader {
    ((val as usize) & ABA_PTR_MASK) as *mut OxHeader
}

#[inline(always)]
fn head_tag(val: *mut OxHeader) -> usize {
    (val as usize) & ABA_TAG_MASK
}

pub struct InternalCache {
    pub list: *mut *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub usage: *mut *mut [AtomicUsize; NUM_SIZE_CLASSES],
    pub pushed: *mut *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub trimmed: *mut *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub has_pushed: *mut *mut AtomicBool,
}

pub struct InterConnectCache {
    pub cache: InternalCache,
    pub ncpu: usize,
    pub node_count: usize,
}

impl InterConnectCache {
    pub fn new(ncpu: usize) -> Self {
        let list = (0..ncpu)
            .map(|_| {
                let arr: Box<[AtomicPtr<OxHeader>; NUM_SIZE_CLASSES]> =
                    Box::new(std::array::from_fn(|_| AtomicPtr::new(null_mut())));
                Box::into_raw(arr)
            })
            .collect::<Vec<_>>();

        let usage = (0..ncpu)
            .map(|_| {
                let arr: Box<[AtomicUsize; NUM_SIZE_CLASSES]> =
                    Box::new(std::array::from_fn(|_| AtomicUsize::new(0)));
                Box::into_raw(arr)
            })
            .collect::<Vec<_>>();

        let pushed = (0..ncpu)
            .map(|_| {
                let arr: Box<[AtomicPtr<OxHeader>; NUM_SIZE_CLASSES]> =
                    Box::new(std::array::from_fn(|_| AtomicPtr::new(null_mut())));
                Box::into_raw(arr)
            })
            .collect::<Vec<_>>();

        let trimmed = (0..ncpu)
            .map(|_| {
                let arr: Box<[AtomicPtr<OxHeader>; NUM_SIZE_CLASSES]> =
                    Box::new(std::array::from_fn(|_| AtomicPtr::new(null_mut())));
                Box::into_raw(arr)
            })
            .collect::<Vec<_>>();

        let has_pushed = (0..ncpu)
            .map(|_| Box::into_raw(Box::new(AtomicBool::new(false))))
            .collect::<Vec<_>>();

        let list_ptr = Box::into_raw(list.into_boxed_slice())
            as *mut *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
        let usage_ptr =
            Box::into_raw(usage.into_boxed_slice()) as *mut *mut [AtomicUsize; NUM_SIZE_CLASSES];
        let pushed_ptr = Box::into_raw(pushed.into_boxed_slice())
            as *mut *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
        let trimmed_ptr = Box::into_raw(trimmed.into_boxed_slice())
            as *mut *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
        let has_pushed_ptr = Box::into_raw(has_pushed.into_boxed_slice()) as *mut *mut AtomicBool;

        InterConnectCache {
            cache: InternalCache {
                list: list_ptr,
                usage: usage_ptr,
                pushed: pushed_ptr,
                trimmed: trimmed_ptr,
                has_pushed: has_pushed_ptr,
            },
            ncpu,
            node_count: 1,
        }
    }

    pub unsafe fn try_push(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
        need_push_pushed: bool,
        is_trimmed: bool,
        thread_id: usize,
    ) {
        let usage = unsafe { &*(*self.cache.usage.add(thread_id)) };

        let list_ptr = if unlikely(is_trimmed) {
            unsafe { &*(*self.cache.trimmed.add(thread_id)) }
        } else if need_push_pushed {
            let has_pushed = unsafe { &*(*self.cache.has_pushed.add(thread_id)) };
            if !has_pushed.load(Ordering::Relaxed) {
                let _ =
                    has_pushed.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed);
            }
            unsafe { &*(*self.cache.pushed.add(thread_id)) }
        } else {
            unsafe { &*(*self.cache.list.add(thread_id)) }
        };

        let mut current_head = list_ptr[class].load(Ordering::Relaxed);

        loop {
            unsafe { (*tail).next = head_ptr(current_head) };

            match list_ptr[class].compare_exchange_weak(
                current_head,
                head_pack(
                    unsafe { xor_ptr_general(head, NUMA_KEY) },
                    head_tag(current_head).wrapping_add(1),
                ),
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    usage[class].fetch_add(batch_size, Ordering::Relaxed);
                    break;
                }
                Err(actual) => current_head = actual,
            }
        }
    }

    pub unsafe fn try_pop(
        &self,
        class: usize,
        batch_size: usize,
        need_pushed: bool,
        cpu: usize,
    ) -> *mut OxHeader {
        if let Some(popped) = unsafe { self.pop(class, batch_size, cpu, need_pushed) } {
            return popped;
        }

        for i in 1..self.ncpu {
            let victim = (cpu + i) % self.ncpu;
            if let Some(block) = unsafe { self.pop(class, batch_size, victim, need_pushed) } {
                return block;
            }
        }

        null_mut()
    }

    pub unsafe fn pop(
        &self,
        class: usize,
        batch_size: usize,
        thread_id: usize,
        need_pushed: bool,
    ) -> Option<*mut OxHeader> {
        let usage = unsafe { &*(*self.cache.usage.add(thread_id)) };

        if usage[class].load(Ordering::Relaxed) == 0 {
            return None;
        }

        let mut list_ptr = unsafe { &*(*self.cache.list.add(thread_id)) };

        let mut tried_trim_list = false;
        let mut tried_pushed = false;

        loop {
            let cur = list_ptr[class].load(Ordering::Relaxed);
            let head_enc = cur;

            if unlikely(head_ptr(head_enc).is_null()) {
                let has_pushed = unsafe { &*(*self.cache.has_pushed.add(thread_id)) };
                if has_pushed.load(Ordering::Relaxed) && need_pushed && !tried_pushed {
                    list_ptr = unsafe { &*(*self.cache.pushed.add(thread_id)) };
                    tried_pushed = true;
                    continue;
                }

                if tried_pushed && !tried_trim_list {
                    let _ = has_pushed.compare_exchange(
                        true,
                        false,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                }

                if !tried_trim_list && need_pushed {
                    list_ptr = unsafe { &*(*self.cache.trimmed.add(thread_id)) };
                    tried_trim_list = true;
                    continue;
                }

                return None;
            }

            let head = unsafe { xor_ptr_general(head_ptr(head_enc), NUMA_KEY) };

            let mut tail = head;
            let mut count = 1;
            for _ in 1..batch_size {
                let next_enc = unsafe { (*tail).next };
                if unlikely(next_enc.is_null()) {
                    break;
                }
                tail = unsafe { xor_ptr_general(next_enc, NUMA_KEY) };
                count += 1;
            }

            let new_head_enc = unsafe { (*tail).next };

            if list_ptr[class]
                .compare_exchange(
                    cur,
                    head_pack(new_head_enc, head_tag(cur).wrapping_add(1)),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                usage[class].fetch_sub(count, Ordering::Relaxed);
                unsafe { (*tail).next = null_mut() };
                return Some(head);
            }
        }
    }
}

struct IccWrap(*const InterConnectCache);
unsafe impl Send for IccWrap {}

#[test]
fn test_icc_basic() {
    loom::model(|| {
        let ncpu = 2;
        let icc = InterConnectCache::new(ncpu);
        let wrap = IccWrap(&icc);

        let t1 = thread::spawn(move || {
            let icc = unsafe { &*wrap.0 };
            let mut h1 = OxHeader {
                next: null_mut(),
                class: 0,
                magic: 0,
                life_time: 0,
            };
            unsafe { icc.try_push(0, &mut h1, &mut h1, 1, false, false, 0) };

            let popped = unsafe { icc.try_pop(0, 1, false, 0) };
            assert!(!popped.is_null());
            assert_eq!(popped, &mut h1 as *mut _);
        });

        let t2 = thread::spawn(move || {
            let icc = unsafe { &*wrap.0 };
            let mut h2 = OxHeader {
                next: null_mut(),
                class: 0,
                magic: 0,
                life_time: 0,
            };
            unsafe { icc.try_push(0, &mut h2, &mut h2, 1, false, false, 1) };

            let popped = unsafe { icc.try_pop(0, 1, false, 1) };
            assert!(!popped.is_null());
            assert_eq!(popped, &mut h2 as *mut _);
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
fn test_icc_stealing() {
    loom::model(|| {
        let ncpu = 2;
        let icc = InterConnectCache::new(ncpu);
        let wrap = IccWrap(&icc);

        let t1 = thread::spawn(move || {
            let icc = unsafe { &*wrap.0 };
            let mut h1 = OxHeader {
                next: null_mut(),
                class: 0,
                magic: 0,
                life_time: 0,
            };
            unsafe { icc.try_push(0, &mut h1, &mut h1, 1, false, false, 0) };
        });

        let t2 = thread::spawn(move || {
            let icc = unsafe { &*wrap.0 };
            let popped = unsafe { icc.try_pop(0, 1, false, 1) };
            if !popped.is_null() {}
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
fn test_icc_high_load() {
    let mut builder = loom::model::Builder::new();
    builder.max_threads = 12;
    builder.check(|| {
        let ncpu = 4;
        let icc = InterConnectCache::new(ncpu);
        let wrap = IccWrap(&icc);

        let mut threads = vec![];
        for i in 0..4 {
            let wrap = IccWrap(wrap.0);
            let cpu = i % ncpu;
            threads.push(thread::spawn(move || {
                let icc = unsafe { &*wrap.0 };
                let mut h = OxHeader {
                    next: null_mut(),
                    class: 0,
                    magic: 0,
                    life_time: 0,
                };
                unsafe {
                    icc.try_push(0, &mut h, &mut h, 1, false, false, cpu);
                    let popped = icc.try_pop(0, 1, false, cpu);
                    assert!(!popped.is_null());
                }
            }));
        }

        for t in threads {
            t.join().unwrap();
        }
    });
}

#[test]
fn test_icc_brutal() {
    let mut builder = loom::model::Builder::new();
    builder.max_threads = 12;
    builder.max_branches = 100000;
    builder.check(|| {
        let ncpu = 4;
        let icc = InterConnectCache::new(ncpu);
        let wrap = IccWrap(&icc);

        let mut headers = (0..40)
            .map(|_| OxHeader {
                next: null_mut(),
                class: 0,
                magic: 0xEE,
                life_time: 0,
            })
            .collect::<Vec<_>>();
        let headers_ptr = headers.as_mut_ptr();

        let mut threads = vec![];
        for i in 0..4 {
            let wrap = IccWrap(wrap.0);
            let cpu = i % ncpu;
            let offset = i * 10;

            threads.push(thread::spawn(move || {
                let icc = unsafe { &*wrap.0 };

                for it in 0..4 {
                    let class = (i + it) % 3;
                    unsafe {
                        for k in 0..5 {
                            let h = headers_ptr.add(offset + k);
                            icc.try_push(class, h, h, 1, false, false, cpu);
                            if k % 2 == 0 {
                                thread::yield_now();
                            }
                        }

                        let _ = icc.try_pop(class, 2, false, cpu);
                        thread::yield_now();

                        for k in 5..10 {
                            let h = headers_ptr.add(offset + k);
                            icc.try_push(class, h, h, 1, false, false, cpu);
                        }

                        let _ = icc.try_pop(class, 8, false, cpu);
                    }
                }
            }));
        }

        for t in threads {
            t.join().unwrap();
        }
    });
}
