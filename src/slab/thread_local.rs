use std::{
    cell::UnsafeCell,
    hint::{likely, unlikely},
    os::raw::c_void,
    ptr::null_mut,
};

#[cfg(feature = "hardened-linked-list")]
use crate::sys::memory_system::getrandom;
use crate::{
    MetaData, OxHeader, OxidallocError,
    slab::{NUM_SIZE_CLASSES, bulk_allocation::drain_pending, interconnect::ICC, xor_ptr_general},
    sys::memory_system::{MMapFlags, MProtFlags, MemoryFlags, mmap_memory, unmap_memory},
    va::is_ours,
};

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

struct CleanupHandler;

impl Drop for CleanupHandler {
    fn drop(&mut self) {
        unsafe { cleanup_thread_cache(TLS) };
    }
}

#[repr(C)]
pub struct TlsBin {
    pub head: *mut OxHeader,
    pub usage: usize,
}

#[repr(C, align(64))]
pub struct ThreadLocalEngine {
    pub tls: [TlsBin; NUM_SIZE_CLASSES],
    pub pending: [*mut MetaData; NUM_SIZE_CLASSES],
    #[cfg(feature = "hardened-linked-list")]
    pub xor_key: usize,
}

#[thread_local]
pub static mut TLS: *mut ThreadLocalEngine = null_mut();
#[thread_local]
static TLS_DESTRUCTOR: UnsafeCell<Option<CleanupHandler>> = UnsafeCell::new(None);

#[inline(always)]
unsafe fn touch_tls() {
    let slot = TLS_DESTRUCTOR.get();
    core::ptr::write_volatile(slot, Some(CleanupHandler));
    core::ptr::read_volatile(slot);
}

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
    #[inline(never)]
    pub unsafe fn init_tls() -> *mut ThreadLocalEngine {
        let total_size = size_of::<ThreadLocalEngine>();

        let cache = mmap_memory(
            null_mut(),
            total_size,
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        )
        .unwrap_or_else(|_| {
            OxidallocError::PThreadCacheFailed.log_and_abort(
                null_mut(),
                "PThread cache creation failed: errno({})",
                None,
            )
        }) as *mut ThreadLocalEngine;

        #[cfg(feature = "hardened-linked-list")]
        let rand_s: usize;
        #[cfg(feature = "hardened-linked-list")]
        {
            let mut rand_slice = [0u8; 8];
            let res = getrandom(&mut rand_slice);
            if res.is_err() {
                OxidallocError::SecurityViolation.log_and_abort(
                    null_mut(),
                    "Failed to generate per-thread encryption key",
                    None,
                );
            }
            rand_s = usize::from_ne_bytes(rand_slice);
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
                #[cfg(feature = "hardened-linked-list")]
                xor_key: rand_s,
            },
        );

        touch_tls();
        TLS = cache;

        cache
    }

    pub unsafe fn get_or_init() -> &'static mut ThreadLocalEngine {
        if likely(!TLS.is_null()) {
            return &mut *TLS;
        }

        Self::get_or_init_cold()
    }

    #[inline(never)]
    #[cold]
    pub unsafe fn get_or_init_cold() -> &'static mut ThreadLocalEngine {
        TLS = Self::init_tls();

        &mut *TLS
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
        #[cfg(not(feature = "hardened-linked-list"))]
        if likely(!next.is_null()) {
            prefetch(next as *const u8);
        }
        #[cfg(feature = "hardened-linked-list")]
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

    pub unsafe fn pop_batch(
        &mut self,
        class: usize,
        batch_size: usize,
    ) -> (*mut OxHeader, *mut OxHeader, usize) {
        if unlikely(self.tls[class].usage == 0) {
            return (null_mut(), null_mut(), 0);
        }

        let real_batch = batch_size.min(self.tls[class].usage);
        let head_enc = self.tls[class].head;
        let head = self.xor_ptr(head_enc);

        let mut tail = head;
        for _ in 1..real_batch {
            let next_enc = (*tail).next;
            if next_enc.is_null() {
                break;
            }
            tail = self.xor_ptr(next_enc);
        }

        let new_head_enc = (*tail).next;
        (*tail).next = null_mut();

        self.tls[class].head = new_head_enc;
        self.tls[class].usage -= real_batch;

        #[cfg(feature = "hardened-linked-list")]
        {
            let mut curr = head;
            while curr != tail {
                let next_enc = (*curr).next;
                let next_raw = self.xor_ptr(next_enc);
                (*curr).next = next_raw;
                curr = next_raw;
            }
        }

        (head, tail, real_batch)
    }
}

unsafe fn cleanup_thread_cache(cache: *mut ThreadLocalEngine) {
    TLS = null_mut();
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

            ICC.try_push(class, head, tail, count, false, false);
        }

        drain_pending(&mut *cache, class);
    }

    let total_size = size_of::<ThreadLocalEngine>();

    let _ = unmap_memory(cache as *mut c_void, total_size);
}

#[cfg(test)]
mod tests {
    use std::{hint::black_box, time::Instant};

    use crate::{FREED_MAGIC, sys::memory_system::MProtFlags};

    use super::*;

    #[test]
    fn tls_speed_test() {
        unsafe {
            let tls = ThreadLocalEngine::get_or_init();
            let start = Instant::now();
            let header = mmap_memory(
                null_mut(),
                size_of::<OxHeader>(),
                MMapFlags {
                    prot: MProtFlags::READ | MProtFlags::WRITE,
                    map: MemoryFlags::PRIVATE,
                },
            )
            .unwrap() as *mut OxHeader;
            std::ptr::write(
                header,
                OxHeader {
                    next: null_mut(),
                    class: 0,
                    magic: FREED_MAGIC,
                    life_time: 0,
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
