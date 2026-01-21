#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "hardened-linked-list")]
use libc::getrandom;
use libc::pthread_setspecific;
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};
use std::{
    hint::{likely, unlikely},
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
};

use crate::{
    MAX_NUMA_NODES, MetaData, OxHeader, OxidallocError,
    slab::{NUM_SIZE_CLASSES, global::GlobalHandler, xor_ptr_general},
    va::is_ours,
};

pub static TOTAL_THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

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
    pub head: *mut OxHeader,
    pub usage: usize,
}

#[repr(C, align(64))]
pub struct ThreadLocalEngine {
    pub tls: [TlsBin; NUM_SIZE_CLASSES],
    pub pending: [*mut MetaData; NUM_SIZE_CLASSES],
    pub numa_node_id: usize,
    #[cfg(feature = "hardened-linked-list")]
    pub xor_key: usize,
}

#[thread_local]
pub static mut TLS: *mut ThreadLocalEngine = null_mut();

impl ThreadLocalEngine {
    #[inline(always)]
    pub unsafe fn xor_ptr(&self, ptr: *mut OxHeader) -> *mut OxHeader {
        #[cfg(feature = "hardened-linked-list")]
        {
            if unlikely(ptr.is_null()) {
                return null_mut();
            }
            ((ptr as usize) ^ self.xor_key) as *mut OxHeader
        }

        #[cfg(not(feature = "hardened-linked-list"))]
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

        #[cfg(feature = "hardened-linked-list")]
        let mut rand: usize = 0;
        #[cfg(feature = "hardened-linked-list")]
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
                        head: null_mut(),
                        usage: 0,
                    }
                }; NUM_SIZE_CLASSES],
                pending: [const { null_mut() }; NUM_SIZE_CLASSES],
                numa_node_id: (numa % MAX_NUMA_NODES),
                #[cfg(feature = "hardened-linked-list")]
                xor_key: rand,
            },
        );

        #[cfg(feature = "hardened-linked-list")]
        {
            let _ = rustix::mm::madvise(
                cache as *mut c_void,
                total_size,
                rustix::mm::Advice::LinuxDontDump,
            );
        }

        pthread_setspecific(key, cache as *mut c_void);
        TLS = cache;

        cache
    }

    #[inline(always)]
    pub unsafe fn get_or_init() -> &'static mut ThreadLocalEngine {
        if likely(!TLS.is_null()) {
            return &mut *TLS;
        }

        Self::get_or_init_cold()
    }

    #[inline(never)]
    #[cold]
    pub unsafe fn get_or_init_cold() -> &'static mut ThreadLocalEngine {
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

        &mut *tls
    }

    #[inline(always)]
    pub unsafe fn pop_from_thread(&mut self, class: usize) -> *mut OxHeader {
        let bin = &mut self.tls[class];

        let current_header = bin.head;
        if unlikely(current_header.is_null()) {
            return null_mut();
        }

        let header = self.xor_ptr(current_header);

        let next = (*header).next;
        if likely(!next.is_null()) {
            prefetch(self.xor_ptr(next) as *const u8);
        }

        self.tls[class].head = next;
        self.tls[class].usage -= 1;

        header
    }

    #[inline(always)]
    pub unsafe fn push_to_thread(&mut self, class: usize, head: *mut OxHeader) {
        let bin = &self.tls[class];

        let current = bin.head;
        (*head).next = current;

        self.tls[class].head = self.xor_ptr(head);
        self.tls[class].usage += 1;
    }

    #[inline(always)]
    pub unsafe fn push_to_thread_tailed(
        &mut self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
    ) {
        let bin = &self.tls[class];

        #[cfg(feature = "hardened-linked-list")]
        {
            let mut curr = head;
            while curr != tail {
                let next_raw = (*curr).next;
                (*curr).next = self.xor_ptr(next_raw);
                curr = next_raw;
            }
        }

        let current_header = bin.head;
        (*tail).next = current_header;

        self.tls[class].head = self.xor_ptr(head);
        self.tls[class].usage += batch_size;
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

    #[cfg(feature = "hardened-linked-list")]
    let random_key = (*cache).xor_key;
    #[cfg(not(feature = "hardened-linked-list"))]
    let random_key = 0;

    for class in 0..NUM_SIZE_CLASSES {
        let head = xor_ptr_general((*cache).tls[class].head, random_key);

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

#[cfg(test)]
mod tests {
    use std::{hint::black_box, time::Instant};

    use crate::FREED_MAGIC;

    use super::*;

    #[test]
    fn tls_speed_test() {
        unsafe {
            let tls = ThreadLocalEngine::get_or_init();
            let start = Instant::now();
            let header = mmap_anonymous(
                null_mut(),
                size_of::<OxHeader>(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
            .unwrap() as *mut OxHeader;
            std::ptr::write(
                header,
                OxHeader {
                    next: null_mut(),
                    size: size_of::<OxHeader>(),
                    class: 0,
                    magic: FREED_MAGIC,
                    life_time: 0,
                    metadata: null_mut(),
                },
            );
            tls.push_to_thread(1, header);

            for _ in 0..1000000 {
                let header = black_box(tls.pop_from_thread(1));
                black_box(tls.push_to_thread(1, header));
            }
            let end = Instant::now();
            let dur = end - start;
            let ns = dur.as_nanos() as f64 / 1_000_000.0;
            println!("TLS pop+push: {:.2} ns/op", ns);
        }
    }

    #[test]
    fn init_speed_test() {
        unsafe {
            const N: usize = 10_000_000;
            let _tls = ThreadLocalEngine::get_or_init();
            _tls.numa_node_id = 0;

            let start = Instant::now();
            for _ in 0..N {
                black_box(ThreadLocalEngine::get_or_init());
            }
            let end = Instant::now();
            let ns = end.duration_since(start).as_nanos() as f64 / N as f64;
            println!("Get speed: {:.2} ns/op", ns);
        }
    }
}
