#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "hardened")]
use libc::getrandom;
use libc::pthread_setspecific;
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};
use std::{
    hint::{likely, unlikely},
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering},
    },
};

use crate::{
    MAX_NUMA_NODES, MetaData, OxHeader, OxidallocError,
    slab::{NUM_SIZE_CLASSES, global::GlobalHandler, xor_ptr_general},
    va::is_ours,
};

pub static TOTAL_THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct ThreadNode {
    pub engine: AtomicPtr<ThreadLocalEngine>,
    pub next: AtomicPtr<ThreadNode>,
}

pub static THREAD_REGISTER: AtomicPtr<ThreadNode> = AtomicPtr::new(null_mut());

unsafe fn try_reuse_node(ptr: *mut ThreadLocalEngine) -> Option<*mut ThreadNode> {
    let mut node = THREAD_REGISTER.load(Ordering::Acquire);
    while !node.is_null() {
        if (*node).engine.load(Ordering::Acquire).is_null()
            && (*node)
                .engine
                .compare_exchange(null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            return Some(node);
        }

        node = (*node).next.load(Ordering::Acquire);
    }

    None
}

// Register a new thread node with the given, need for trimming
unsafe fn register_node(ptr: *mut ThreadLocalEngine) -> *mut ThreadNode {
    if let Some(node) = try_reuse_node(ptr) {
        return node;
    }

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
    TOTAL_THREAD_COUNT.store(cpu, Ordering::Relaxed);

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

pub unsafe fn get_or_init_key() {
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
}

#[repr(C, align(64))]
pub struct ThreadLocalEngine {
    pub tls: [TlsBin; NUM_SIZE_CLASSES],
    pub pending: [AtomicPtr<MetaData>; NUM_SIZE_CLASSES],
    pub node: *mut ThreadNode,
    pub numa_node_id: usize,
    #[cfg(feature = "hardened")]
    pub xor_key: usize,
}

#[thread_local]
pub static mut TLS: *mut ThreadLocalEngine = null_mut();

impl ThreadLocalEngine {
    #[inline(always)]
    pub unsafe fn xor_ptr(&self, ptr: *mut OxHeader) -> *mut OxHeader {
        #[cfg(feature = "hardened")]
        {
            if unlikely(ptr.is_null()) {
                return null_mut();
            }
            ((ptr as usize) ^ self.xor_key) as *mut OxHeader
        }

        #[cfg(not(feature = "hardened"))]
        {
            ptr
        }
    }

    #[cold]
    pub unsafe fn init_tls(key: u32) -> *mut ThreadLocalEngine {
        let total_size = size_of::<ThreadLocalEngine>();

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

        #[cfg(feature = "hardened")]
        let mut rand: usize = 0;
        #[cfg(feature = "hardened")]
        {
            let res = getrandom(
                &mut rand as *mut usize as *mut c_void,
                size_of::<usize>(),
                0,
            );
            if res as usize != size_of::<usize>() {
                OxidallocError::SecurityViolation.log_and_abort(
                    null_mut(),
                    "Failed to generate per-thread encryption key",
                    None,
                );
            }
        }

        // To register TLS write the needed areas, and write NUMA node ID so we can use it for numa allocation
        std::ptr::write(
            cache,
            ThreadLocalEngine {
                tls: [const {
                    TlsBin {
                        head: AtomicPtr::new(null_mut()),
                        usage: AtomicUsize::new(0),
                    }
                }; NUM_SIZE_CLASSES],
                pending: [const { AtomicPtr::new(null_mut()) }; NUM_SIZE_CLASSES],
                node: null_mut(),
                numa_node_id: (numa % MAX_NUMA_NODES),
                #[cfg(feature = "hardened")]
                xor_key: rand,
            },
        );

        #[cfg(feature = "hardened")]
        {
            let _ = rustix::mm::madvise(
                cache as *mut c_void,
                total_size,
                rustix::mm::Advice::LinuxDontDump,
            );
        }

        (*cache).node = register_node(cache);
        pthread_setspecific(key, cache as *mut c_void);
        TLS = cache;

        cache
    }

    #[inline(always)]
    pub unsafe fn get_or_init() -> &'static ThreadLocalEngine {
        if likely(!TLS.is_null()) {
            return &*TLS;
        }

        Self::get_or_init_cold()
    }

    #[inline(never)]
    #[cold]
    pub unsafe fn get_or_init_cold() -> &'static ThreadLocalEngine {
        let key = if !THREAD_INIT.load(Ordering::Acquire) {
            THREAD_KEY.load(Ordering::Acquire)
        } else {
            get_or_init_key();
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
        let bin = &self.tls[class];

        loop {
            let current_header = bin.head.load(Ordering::Relaxed);
            #[cfg(feature = "hardened")]
            {
                if unlikely(current_header == !0) {
                    return null_mut();
                }
            }

            let header = self.xor_ptr(current_header);

            if unlikely(header.is_null()) {
                return null_mut();
            }

            let next = (*header).next;
            if likely(!next.is_null()) {
                prefetch(self.xor_ptr(next) as *const u8);
            }

            if bin
                .head
                .compare_exchange(current_header, next, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                bin.usage.fetch_sub(1, Ordering::Relaxed);
                return header;
            }
        }
    }

    #[inline(always)]
    pub unsafe fn push_to_thread(&self, class: usize, head: *mut OxHeader) {
        let bin = &self.tls[class];

        loop {
            let current = bin.head.load(Ordering::Relaxed);
            (*head).next = current;

            if bin
                .head
                .compare_exchange(
                    current,
                    self.xor_ptr(head),
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                bin.usage.fetch_add(1, Ordering::Relaxed);
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
        let bin = &self.tls[class];

        #[cfg(feature = "hardened")]
        {
            let mut curr = head;
            while curr != tail {
                let next_raw = (*curr).next;
                (*curr).next = self.xor_ptr(next_raw);
                curr = next_raw;
            }
        }

        loop {
            let current_header = bin.head.load(Ordering::Relaxed);
            (*tail).next = current_header;

            if bin
                .head
                .compare_exchange(
                    current_header,
                    self.xor_ptr(head),
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                bin.usage.fetch_add(batch_size, Ordering::Relaxed);
                return;
            }
        }
    }
}

unsafe extern "C" fn cleanup_thread_cache(cache_ptr: *mut c_void) {
    let key = THREAD_KEY.load(Ordering::Acquire);
    TLS = null_mut();
    pthread_setspecific(key, null_mut());

    let cache = cache_ptr as *mut ThreadLocalEngine;
    if cache.is_null() {
        return;
    }
    destroy_node((*cache).node);

    #[cfg(feature = "hardened")]
    let random_key = (*cache).xor_key;
    #[cfg(not(feature = "hardened"))]
    let random_key = 0;

    for class in 0..NUM_SIZE_CLASSES {
        let head = xor_ptr_general(
            (*cache).tls[class].head.swap(null_mut(), Ordering::AcqRel),
            random_key,
        );

        if !is_ours(head as usize) {
            continue;
        }

        if !head.is_null() {
            let mut tail = head;
            let mut count = 1;
            loop {
                let next_encrypted = (*tail).next;
                let next = xor_ptr_general(next_encrypted, random_key);

                (*tail).next = next;
                (*tail).life_time = 0;

                if next.is_null() {
                    break;
                }

                if !is_ours(next as usize) {
                    (*tail).next = null_mut();
                    break;
                }

                tail = next;
                count += 1;
            }

            GlobalHandler.push_to_global(class, (*cache).numa_node_id, head, tail, count);
        }
    }

    let total_size = size_of::<ThreadLocalEngine>();

    let _ = munmap(cache_ptr, total_size);
}
