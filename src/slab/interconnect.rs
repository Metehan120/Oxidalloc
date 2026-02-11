// This implementation does not intended to be used with RSEQ
// Not every kernel supports RSEQ, so we need to handle with sched_getcpu() for kernel compatibility

use std::{
    hint::unlikely,
    os::raw::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

#[cfg(feature = "hardened-linked-list")]
use crate::internals::lock::GlobalLock;
use crate::{
    MAX_NUMA_NODES, OxHeader, OxidallocError,
    internals::once::Once,
    slab::{NUM_SIZE_CLASSES, thread_local::prefetch, xor_ptr_general},
    sys::{
        memory_system::{MMapFlags, MProtFlags, MemoryFlags, get_cpu_count, mmap_memory},
        numa::{MAX_CPUS, get_numa_maps},
    },
    va::bootstrap::NUMA_KEY,
};
use rustix::thread::sched_getcpu;

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

#[thread_local]
static mut CACHED_CPU_ID: usize = 0;
#[thread_local]
static mut ALLOC_COUNT: u32 = 0;

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

pub struct InternalCache {
    pub list: *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub usage: *mut [AtomicUsize; NUM_SIZE_CLASSES],
    pub pushed: *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub trimmed: *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES],
    pub has_pushed: *mut AtomicBool,
}

#[repr(C, align(64))]
pub struct InterConnectCache {
    pub cache: InternalCache,
    #[cfg(feature = "hardened-linked-list")]
    pub locks: *mut GlobalLock,
    pub ncpu: usize,
    pub node_offsets: *mut usize,
    pub node_cpus: *mut usize,
    pub node_count: usize,
    pub node_cpu_count: usize,
    pub cpu_to_node: *mut usize,
    pub once: Once,
}

impl InterConnectCache {
    pub const fn new() -> Self {
        InterConnectCache {
            cache: InternalCache {
                list: null_mut(),
                usage: null_mut(),
                pushed: null_mut(),
                has_pushed: null_mut(),
                trimmed: null_mut(),
            },
            #[cfg(feature = "hardened-linked-list")]
            locks: null_mut(),
            ncpu: 0,
            node_offsets: null_mut(),
            node_cpus: null_mut(),
            node_count: 0,
            node_cpu_count: 0,
            cpu_to_node: null_mut(),
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
            let trimmed = mmap!(
                null_mut(),
                size_of::<[AtomicPtr<OxHeader>; NUM_SIZE_CLASSES]>() * thread_count
            );
            let pushed = mmap!(
                null_mut(),
                size_of::<[AtomicPtr<OxHeader>; NUM_SIZE_CLASSES]>() * thread_count
            );
            let has_pushed = mmap!(null_mut(), size_of::<AtomicBool>() * thread_count);
            let usage = mmap!(
                null_mut(),
                size_of::<[AtomicUsize; NUM_SIZE_CLASSES]>() * thread_count
            );
            #[cfg(feature = "hardened-linked-list")]
            let locks = mmap!(null_mut(), size_of::<GlobalLock>() * thread_count);

            self.cache.list = list as *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
            self.cache.usage = usage as *mut [AtomicUsize; NUM_SIZE_CLASSES];
            self.cache.pushed = pushed as *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
            self.cache.trimmed = trimmed as *mut [AtomicPtr<OxHeader>; NUM_SIZE_CLASSES];
            self.cache.has_pushed = has_pushed as *mut AtomicBool;

            #[cfg(feature = "hardened-linked-list")]
            {
                self.locks = locks as *mut GlobalLock
            };
            self.ncpu = thread_count.max(1);

            if let Some(maps) = get_numa_maps(MAX_NUMA_NODES) {
                self.node_offsets = maps.node_offsets;
                self.node_cpus = maps.node_cpus;
                self.cpu_to_node = maps.cpu_to_node;
                self.node_count = maps.node_count;
                self.node_cpu_count = maps.node_cpu_count;
            }
        });
    }

    #[inline(always)]
    pub unsafe fn get_cpu_fast(&self) -> usize {
        if ALLOC_COUNT % 2 == 0 {
            CACHED_CPU_ID = sched_getcpu() % self.ncpu;
        }
        ALLOC_COUNT += 1;
        CACHED_CPU_ID
    }

    #[inline(always)]
    pub unsafe fn try_push(
        &mut self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
        need_push_pushed: bool,
        is_trimmed: bool,
    ) {
        self.ensure_cache();
        let thread_id = self.get_cpu_fast();

        #[cfg(feature = "hardened-linked-list")]
        let lock = &*self.locks.add(thread_id);
        #[cfg(feature = "hardened-linked-list")]
        let _guard = lock.lock(class);

        let usage = &*self.cache.usage.add(thread_id);

        #[cfg(feature = "debug")]
        INTER.fetch_add(1, Ordering::Relaxed);

        let list = if unlikely(is_trimmed) {
            &*self.cache.trimmed.add(thread_id)
        } else if need_push_pushed {
            let has_pushed = &*self.cache.has_pushed.add(thread_id);
            if !has_pushed.load(Ordering::Relaxed) {
                let _ = has_pushed.compare_exchange_weak(
                    false,
                    true,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            &*self.cache.pushed.add(thread_id)
        } else {
            &*self.cache.list.add(thread_id)
        };

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
    }

    pub unsafe fn get_size(&mut self, class: usize) -> usize {
        self.ensure_cache();

        let mut total = 0;
        for i in 0..self.ncpu {
            let usage = &*self.cache.usage.add(i);
            total += usage[class].load(Ordering::Relaxed);
        }
        total
    }

    #[inline(always)]
    pub unsafe fn try_pop(
        &mut self,
        class: usize,
        batch_size: usize,
        need_pushed: bool,
    ) -> *mut OxHeader {
        self.ensure_cache();
        let cpu = self.get_cpu_fast();

        let ncpu = self.ncpu;

        if let Some(popped) = self.pop(class, batch_size, cpu, need_pushed) {
            return popped;
        }

        if self.node_count > 1 && !self.cpu_to_node.is_null() && cpu < ncpu {
            let cpu_to_node_len = ncpu.min(MAX_CPUS);
            let cpu_to_node = core::slice::from_raw_parts(self.cpu_to_node, cpu_to_node_len);
            let node = cpu_to_node[cpu];
            if node < self.node_count {
                let offsets = core::slice::from_raw_parts(self.node_offsets, self.node_count + 1);
                let cpus = core::slice::from_raw_parts(self.node_cpus, self.node_cpu_count);
                let start = offsets[node];
                let end = offsets[node + 1];

                for &victim in &cpus[start..end] {
                    if victim == cpu || victim >= ncpu {
                        continue;
                    }
                    if let Some(block) = self.pop(class, batch_size, victim, need_pushed) {
                        return block;
                    }
                }
            }
        }

        for i in 1..ncpu {
            let victim = (cpu + i) % ncpu;

            if let Some(block) = self.pop(class, batch_size, victim, need_pushed) {
                return block;
            }
        }

        null_mut()
    }

    #[inline(always)]
    pub unsafe fn pop(
        &mut self,
        class: usize,
        batch_size: usize,
        thread_id: usize,
        need_pushed: bool,
    ) -> Option<*mut OxHeader> {
        #[cfg(feature = "hardened-linked-list")]
        let _guard = (*self.locks.add(thread_id)).lock(class);
        let usage = &*self.cache.usage.add(thread_id);

        if usage[class].load(Ordering::Relaxed) == 0 {
            return None;
        }

        #[cfg(feature = "debug")]
        INTER.fetch_add(1, Ordering::Relaxed);
        let mut list = &*self.cache.list.add(thread_id);

        let mut tried_trim_list = false;
        let mut tried_pushed = false;

        loop {
            let cur = list[class].load(Ordering::Relaxed);
            let head_enc = cur;

            if unlikely(head_ptr(head_enc).is_null()) {
                let has_pushed = &*self.cache.has_pushed.add(thread_id);
                if has_pushed.load(Ordering::Relaxed) && need_pushed && !tried_pushed {
                    list = &*self.cache.pushed.add(thread_id);
                    tried_pushed = true;
                    continue;
                }

                if tried_pushed && !tried_trim_list {
                    let _ = has_pushed.compare_exchange_weak(
                        true,
                        false,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                }

                if !tried_trim_list && need_pushed {
                    list = &*self.cache.trimmed.add(thread_id);
                    tried_trim_list = true;
                    continue;
                }

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
