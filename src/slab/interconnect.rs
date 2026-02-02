// This implementation does not intended to be used with RSEQ
// Not every kernel supports RSEQ, so we need to handle with sched_getcpu() for kernel compatibility

use std::{
    hint::unlikely,
    os::raw::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use rustix::thread::sched_getcpu;

#[cfg(feature = "hardened-linked-list")]
use crate::internals::lock::GlobalLock;
use crate::{
    OxHeader, OxidallocError,
    internals::once::Once,
    slab::{NUM_SIZE_CLASSES, thread_local::prefetch, xor_ptr_general},
    sys::memory_system::{MMapFlags, MProtFlags, MemoryFlags, get_cpu_count, mmap_memory},
    va::bootstrap::NUMA_KEY,
};

#[cfg(feature = "debug")]
pub static HIT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "debug")]
pub static INTER: AtomicUsize = AtomicUsize::new(0);

pub static mut ICC: InterConnectCache = InterConnectCache::new();

#[cfg(not(feature = "hardened-linked-list"))]
const ABA_TAG_BITS: usize = 4;
#[cfg(not(feature = "hardened-linked-list"))]
const ABA_TAG_MASK: usize = (1 << ABA_TAG_BITS) - 1;
#[cfg(not(feature = "hardened-linked-list"))]
const ABA_PTR_MASK: usize = !ABA_TAG_MASK;

#[inline(always)]
fn head_pack(ptr: *mut OxHeader, tag: usize) -> *mut OxHeader {
    #[cfg(not(feature = "hardened-linked-list"))]
    {
        ((ptr as usize) | (tag & ABA_TAG_MASK)) as *mut OxHeader
    }
    #[cfg(feature = "hardened-linked-list")]
    {
        let _ = tag;
        ptr
    }
}

#[inline(always)]
fn head_ptr(val: *mut OxHeader) -> *mut OxHeader {
    #[cfg(not(feature = "hardened-linked-list"))]
    {
        ((val as usize) & ABA_PTR_MASK) as *mut OxHeader
    }
    #[cfg(feature = "hardened-linked-list")]
    {
        val
    }
}

#[inline(always)]
fn head_tag(val: *mut OxHeader) -> usize {
    #[cfg(not(feature = "hardened-linked-list"))]
    {
        (val as usize) & ABA_TAG_MASK
    }
    #[cfg(feature = "hardened-linked-list")]
    {
        let _ = val;
        0
    }
}

#[repr(C, align(64))]
pub struct InterConnectCache {
    pub list: *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub usage: *mut [AtomicUsize; NUM_SIZE_CLASSES],
    #[cfg(feature = "hardened-linked-list")]
    pub locks: *mut GlobalLock,
    pub ncpu: usize,
    pub once: Once,
}

macro_rules! mmap {
    ($ptr:expr, $size:expr) => {
        mmap_memory(
            $ptr,
            $size,
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        )
        .unwrap_or_else(|e| {
            OxidallocError::ICCFailedToInitialize.log_and_abort(
                null_mut() as *mut c_void,
                "MMAP call failed",
                Some(e.get_errno()),
            )
        })
    };
}

impl InterConnectCache {
    pub const fn new() -> Self {
        InterConnectCache {
            list: null_mut(),
            usage: null_mut(),
            #[cfg(feature = "hardened-linked-list")]
            locks: null_mut(),
            ncpu: 0,
            once: Once::new(),
        }
    }

    pub unsafe fn ensure_cache(&mut self) {
        self.once.call_once(|| {
            let thread_count = get_cpu_count();
            let list = mmap!(
                null_mut(),
                size_of::<[AtomicPtr<OxHeader>; NUM_SIZE_CLASSES]>() * thread_count
            );
            let usage = mmap!(
                null_mut(),
                size_of::<[AtomicUsize; NUM_SIZE_CLASSES]>() * thread_count
            );
            #[cfg(feature = "hardened-linked-list")]
            let locks = mmap!(null_mut(), size_of::<GlobalLock>() * thread_count);

            self.list = list as *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
            self.usage = usage as *mut [AtomicUsize; NUM_SIZE_CLASSES];
            #[cfg(feature = "hardened-linked-list")]
            {
                self.locks = locks as *mut GlobalLock
            };
            self.ncpu = thread_count;
        });
    }

    pub unsafe fn try_push(
        &mut self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) -> bool {
        self.ensure_cache();
        let thread_id = sched_getcpu();
        #[cfg(feature = "hardened-linked-list")]
        let lock = &*self.locks.add(thread_id);
        #[cfg(feature = "hardened-linked-list")]
        let _guard = lock.lock(class);

        let usage = &*self.usage.add(thread_id);

        #[cfg(feature = "debug")]
        INTER.fetch_add(1, Ordering::Relaxed);

        let list = &*self.list.add(thread_id);
        let mut current_head = list[class].load(Ordering::Relaxed);

        loop {
            (*tail).next = head_ptr(current_head);

            match list[class].compare_exchange_weak(
                current_head,
                head_pack(
                    xor_ptr_general(head, NUMA_KEY),
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

        true
    }

    pub unsafe fn get_size(&self, class: usize) -> usize {
        let mut total = 0;
        for i in 0..self.ncpu {
            let usage = &*self.usage.add(i);
            total += usage[class].load(Ordering::Relaxed);
        }
        total
    }

    pub unsafe fn try_pop(&mut self, class: usize, batch_size: usize) -> *mut OxHeader {
        let cpu = sched_getcpu();
        let ncpu = self.ncpu;

        if let Some(popped) = self.pop(class, batch_size, cpu) {
            return popped;
        }

        for i in 1..ncpu {
            let victim = (cpu + i) % ncpu;

            if let Some(block) = self.pop(class, batch_size, victim) {
                return block;
            }
        }

        null_mut()
    }

    pub unsafe fn pop(
        &mut self,
        class: usize,
        batch_size: usize,
        thread_id: usize,
    ) -> Option<*mut OxHeader> {
        self.ensure_cache();
        #[cfg(feature = "hardened-linked-list")]
        let _guard = (*self.locks.add(thread_id)).lock(class);
        let usage = &*self.usage.add(thread_id);

        if usage[class].load(Ordering::Relaxed) == 0 {
            return None;
        }

        #[cfg(feature = "debug")]
        INTER.fetch_add(1, Ordering::Relaxed);
        let list = &*self.list.add(thread_id);

        loop {
            let cur = list[class].load(Ordering::Relaxed);
            let head_enc = cur;
            if unlikely(head_ptr(head_enc).is_null()) {
                return None;
            }

            let head = xor_ptr_general(head_ptr(head_enc), NUMA_KEY);

            let mut tail = head;
            let mut count = 1;
            for _ in 1..batch_size {
                let next_enc = (*tail).next;
                if unlikely(next_enc.is_null()) {
                    break;
                }
                tail = xor_ptr_general(next_enc, NUMA_KEY);
                count += 1;
            }

            let new_head_enc = (*tail).next;
            if !new_head_enc.is_null() {
                prefetch(xor_ptr_general(new_head_enc, NUMA_KEY) as *const u8);
            }

            if list[class]
                .compare_exchange(
                    cur,
                    head_pack(new_head_enc, head_tag(cur).wrapping_add(1)),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                usage[class].fetch_sub(count, Ordering::Relaxed);

                #[cfg(feature = "hardened-linked-list")]
                {
                    let mut curr = head;
                    while curr != tail {
                        let next_enc = (*curr).next;
                        let next_raw = xor_ptr_general(next_enc, NUMA_KEY);
                        (*curr).next = next_raw;
                        curr = next_raw;
                    }
                }
                (*tail).next = null_mut();

                #[cfg(feature = "debug")]
                let hit = HIT.fetch_add(1, Ordering::Relaxed);

                #[cfg(feature = "debug")]
                if hit % 100 == 0 {
                    let hit = HIT.load(Ordering::Relaxed);
                    let inter = INTER.load(Ordering::Relaxed);
                    let ratio = inter as f64 / (inter as f64 + hit as f64);

                    eprintln!("hit: {}, inter: {}, ratio: {}", hit, inter, ratio);
                }

                return Some(head);
            }
        }
    }

    #[cfg(feature = "hardened-linked-list")]
    pub unsafe fn reset_on_fork(&mut self) {
        if self.locks.is_null() {
            return;
        }

        for i in 0..self.ncpu {
            let lock = &*self.locks.add(i);
            lock.reset_on_fork();
        }
    }
}
