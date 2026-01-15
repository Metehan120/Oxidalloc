#![allow(unsafe_op_in_unsafe_fn)]

use libc::pthread_setspecific;
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};
use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering},
    },
};

use crate::{
    OX_ENABLE_EXPERIMENTAL_HEALING, OxHeader, OxidallocError,
    slab::{
        NUM_SIZE_CLASSES,
        global::{GlobalHandler, MAX_NUMA_NODES},
        quarantine::quarantine,
    },
    va::{bitmap::Segment, is_ours},
};

pub struct ThreadNode {
    pub engine: AtomicPtr<ThreadLocalEngine>,
    pub next: AtomicPtr<ThreadNode>,
}

pub static THREAD_REGISTER: AtomicPtr<ThreadNode> = AtomicPtr::new(null_mut());

// Register a new thread node with the given, need for trimming
unsafe fn register_node(ptr: *mut ThreadLocalEngine) -> *mut ThreadNode {
    let node = match mmap_anonymous(
        null_mut(),
        size_of::<ThreadNode>(),
        ProtFlags::READ | ProtFlags::WRITE,
        MapFlags::PRIVATE,
    ) {
        Ok(mem) => mem,
        Err(err) => OxidallocError::PThreadCacheFailed.log_and_abort(
            0 as *mut c_void,
            "PThread cache creation failed, cannot create thread node: errno({})",
            Some(err),
        ),
    } as *mut ThreadNode;

    std::ptr::write(
        node,
        ThreadNode {
            engine: AtomicPtr::new(ptr),
            next: AtomicPtr::new(null_mut()),
        },
    );

    loop {
        let head = THREAD_REGISTER.load(Ordering::Acquire);
        (*node).next.store(head, Ordering::Relaxed);

        if THREAD_REGISTER
            .compare_exchange(head, node, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            break;
        }
    }

    node
}

unsafe fn destroy_node(node: *mut ThreadNode) {
    (*node).engine.swap(null_mut(), Ordering::AcqRel);
}

unsafe fn get_numa_node_id() -> usize {
    let mut cpu = 0;
    let mut node = 0;

    // get node id so we can use it for numa allocation
    libc::syscall(libc::SYS_getcpu, &mut cpu, &mut node, null_mut::<c_void>());

    node as usize
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub fn prefetch(ptr: *const u8) {
    unsafe { core::arch::x86_64::_mm_prefetch(ptr as *const i8, core::arch::x86_64::_MM_HINT_T0) }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub fn prefetch(ptr: *const u8) {
    unsafe { core::arch::aarch64::_prefetch(ptr, core::arch::aarch64::PLDL1KEEP) }
}

pub unsafe fn get_or_init() {
    THREAD_ONCE.call_once(|| {
        let mut key = 0;
        libc::pthread_key_create(&mut key, Some(cleanup_thread_cache));
        THREAD_KEY.store(key, Ordering::Relaxed);
        THREAD_INIT.store(false, Ordering::Relaxed);
    });
}

static THREAD_KEY: AtomicU32 = AtomicU32::new(0);
static THREAD_ONCE: Once = Once::new();
static THREAD_INIT: AtomicBool = AtomicBool::new(true);

#[repr(C, align(64))]
pub struct TlsBin {
    pub head: AtomicPtr<OxHeader>,
    pub usage: AtomicUsize,
    pub latest_usage: AtomicUsize,
    pub latest_next: AtomicPtr<OxHeader>,
}

pub struct ThreadLocalEngine {
    pub tls: [TlsBin; NUM_SIZE_CLASSES],
    pub node: *mut ThreadNode,
    pub numa_node_id: usize,
    pub latest_segment: AtomicPtr<Segment>,
}

#[thread_local]
static mut TLS: *mut ThreadLocalEngine = null_mut();

impl ThreadLocalEngine {
    pub unsafe fn init_tls(key: u32) -> *mut ThreadLocalEngine {
        let tls_size = size_of::<TlsBin>() * NUM_SIZE_CLASSES;
        let engine_size = size_of::<ThreadLocalEngine>();
        let total_size = tls_size + engine_size;

        let cache = mmap_anonymous(
            null_mut(),
            total_size,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::PRIVATE,
        )
        .unwrap_or_else(|err| {
            OxidallocError::PThreadCacheFailed.log_and_abort(
                null_mut(),
                "PThread cache creation failed: errno({})",
                Some(err),
            )
        }) as *mut ThreadLocalEngine;

        let numa = get_numa_node_id();

        // To register TLS write the needed areas, and write NUMA node ID so we can use it for numa allocation
        std::ptr::write(
            cache,
            ThreadLocalEngine {
                tls: [const {
                    TlsBin {
                        head: AtomicPtr::new(null_mut()),
                        usage: AtomicUsize::new(0),
                        latest_usage: AtomicUsize::new(0),
                        latest_next: AtomicPtr::new(null_mut()),
                    }
                }; NUM_SIZE_CLASSES],
                node: null_mut(),
                numa_node_id: (numa % MAX_NUMA_NODES),
                latest_segment: AtomicPtr::new(null_mut()),
            },
        );

        (*cache).node = register_node(cache);
        pthread_setspecific(key, cache as *mut c_void);
        TLS = cache;

        cache
    }

    #[inline(always)]
    pub unsafe fn get_or_init() -> &'static ThreadLocalEngine {
        if !TLS.is_null() {
            return &*TLS;
        }

        let key = if !THREAD_INIT.load(Ordering::Acquire) {
            THREAD_KEY.load(Ordering::Acquire)
        } else {
            get_or_init();
            THREAD_KEY.load(Ordering::Acquire)
        };

        let mut tls = libc::pthread_getspecific(key) as *mut ThreadLocalEngine;
        if tls.is_null() {
            tls = Self::init_tls(key);
        }
        TLS = tls;

        &*tls
    }

    #[inline(always)]
    pub unsafe fn pop_from_thread(&self, class: usize) -> *mut OxHeader {
        loop {
            let mut header = self.tls[class].head.load(Ordering::Relaxed);

            if header.is_null() {
                return null_mut();
            }

            // Check if the header is ours
            if !is_ours(header as usize, Some(self)) {
                // Try to recover, if fails return null
                if !quarantine(
                    Some(self),
                    header as usize,
                    class,
                    OX_ENABLE_EXPERIMENTAL_HEALING.load(Ordering::Relaxed),
                ) {
                    self.tls[class].usage.store(0, Ordering::Relaxed);
                    self.tls[class].head.store(null_mut(), Ordering::Relaxed);
                    return null_mut();
                }
                header = self.tls[class].head.load(Ordering::Relaxed);
                // Check if data is still valid
                if header.is_null() || !is_ours(header as usize, Some(self)) {
                    return null_mut();
                }
            }

            let next = (*header).next;
            if !next.is_null() {
                prefetch(next as *const u8);
            }

            if self.tls[class]
                .head
                .compare_exchange(header, next, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.tls[class].latest_next.store(next, Ordering::Relaxed);
                let usage = self.tls[class].usage.fetch_sub(1, Ordering::Relaxed);
                self.tls[class].latest_usage.store(usage, Ordering::Relaxed);
                return header;
            }
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread(&self, class: usize, head: *mut OxHeader) {
        loop {
            let current = self.tls[class].head.load(Ordering::Relaxed);
            (*head).next = current;

            if self.tls[class]
                .head
                .compare_exchange(current, head, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                self.tls[class].usage.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread_tailed(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        loop {
            let current_header = self.tls[class].head.load(Ordering::Relaxed);
            (*tail).next = current_header;

            if self.tls[class]
                .head
                .compare_exchange(current_header, head, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                self.tls[class]
                    .usage
                    .fetch_add(batch_size, Ordering::Relaxed);
                return;
            }
        }
    }
}

unsafe extern "C" fn cleanup_thread_cache(cache_ptr: *mut c_void) {
    TLS = null_mut();
    let cache = cache_ptr as *mut ThreadLocalEngine;
    if cache.is_null() {
        return;
    }
    destroy_node((*cache).node);

    // Move all blocks to global
    for class in 0..NUM_SIZE_CLASSES {
        let head = (*cache).tls[class].head.swap(null_mut(), Ordering::AcqRel);
        if !is_ours(head as usize, None) {
            continue;
        }

        if !head.is_null() {
            let mut tail = head;
            let mut count = 1;
            loop {
                let next = (*tail).next;
                (*tail).life_time = 0;
                if next.is_null() {
                    break;
                }
                if !is_ours(next as usize, None) {
                    (*tail).next = null_mut();
                    break;
                }
                tail = next;
                count += 1;
            }

            GlobalHandler.push_to_global(class, (*cache).numa_node_id, head, tail, count);
        }
    }

    let tls_size = size_of::<TlsBin>() * NUM_SIZE_CLASSES;
    let engine_size = size_of::<ThreadLocalEngine>();
    let total_size = tls_size + engine_size;

    let _ = munmap(cache_ptr, total_size);
}
