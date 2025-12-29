#![warn(clippy::nursery, clippy::pedantic)]

use rustix::io::Errno;
use std::{
    fmt::Debug,
    sync::{OnceLock, atomic::AtomicUsize},
    time::Instant,
};

pub mod abi;
pub mod big_allocation;
pub mod slab;
pub mod trim;
pub mod va;

pub enum Err {
    OutOfReservation,
    OutOfMemory,
}

pub const EROCMEF: i32 = 41; // harmless let it stay
pub const VERSION: u32 = 0xABA01;
pub const OX_ALIGN_TAG: usize = u64::from_le_bytes(*b"OXIDALGN") as usize;
pub const MAGIC: u64 = 0x01B01698BF0BEEF;
pub static TOTAL_OPS: AtomicUsize = AtomicUsize::new(0);
pub static OX_GLOBAL_STAMP: OnceLock<Instant> = OnceLock::new();
pub static OX_CURRENT_STAMP: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_IN_USE: AtomicUsize = AtomicUsize::new(0);
pub static AVERAGE_BLOCK_TIMES_PTHREAD: AtomicUsize = AtomicUsize::new(3000);
pub static AVERAGE_BLOCK_TIMES_GLOBAL: AtomicUsize = AtomicUsize::new(3000);

pub fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

pub const HEADER_SIZE: usize = size_of::<OxHeader>();

#[repr(C, align(16))]
pub struct OxHeader {
    pub next: *mut OxHeader,
    pub size: u64,
    pub magic: u64,
    pub flag: i32,
    pub life_time: usize,
    pub in_use: u8,
}

#[repr(u32)]
pub enum OxidallocError {
    DoubleFree = 0x1000,
    MemoryCorruption = 0x1001,
    InvalidSize = 0x1002,
    OutOfMemory = 0x1003,
    VaBitmapExhausted = 0x1004,
    VAIinitFailed = 0x1005,
    PThreadCacheFailed = 0x1006,
    TooMuchQuarantine = 0x1007,
    DoubleQuarantine = 0x1008,
}

impl Debug for OxidallocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OxidallocError::DoubleFree => write!(f, "DoubleFree (0x1000)"),
            OxidallocError::MemoryCorruption => write!(f, "MemoryCorruption (0x1001)"),
            OxidallocError::InvalidSize => write!(f, "InvalidSize (0x1002)"),
            OxidallocError::OutOfMemory => write!(f, "OutOfMemory (0x1003)"),
            OxidallocError::VaBitmapExhausted => write!(f, "VaBitmapExhausted (0x1004)"),
            OxidallocError::VAIinitFailed => write!(f, "VAIinitFailed (0x1005)"),
            OxidallocError::PThreadCacheFailed => write!(f, "PThreadCacheFailed (0x1006)"),
            OxidallocError::TooMuchQuarantine => write!(f, "TooMuchQuarantine (0x1007)"),
            OxidallocError::DoubleQuarantine => write!(f, "DoubleQuarantine (0x1008)"),
        }
    }
}

impl OxidallocError {
    pub fn log_and_abort(
        &self,
        ptr: *mut std::ffi::c_void,
        extra: &str,
        errno: Option<Errno>,
    ) -> ! {
        if let Some(errno) = errno {
            eprintln!(
                "[OXIDALLOC FATAL] {:?} at ptr={:p} | {} | errno({})",
                self,
                ptr,
                extra,
                errno.raw_os_error()
            );
        } else {
            eprintln!("[OXIDALLOC FATAL] {:?} at ptr={:p} | {}", self, ptr, extra);
        }
        std::process::abort();
    }
}

#[test]
fn bench_allocator() {
    unsafe {
        use crate::{abi::free::free, abi::malloc::malloc};
        use std::{hint::black_box, time::Instant};

        let iterations = 1_000_000;

        // Warm up
        for _ in black_box(0..1000) {
            let ptr = black_box(malloc(100));
            black_box(free(ptr));
        }

        // Bench small allocations
        let start = Instant::now();
        for _ in black_box(0..iterations) {
            let ptr = black_box(malloc(100));
            black_box(free(ptr));
        }
        let small_time = start.elapsed();
        println!(
            "Small (100B): {:?} ({:.2} ns/op)",
            small_time,
            small_time.as_nanos() as f64 / iterations as f64
        );

        // Bench medium allocations
        let start = Instant::now();
        for _ in black_box(0..iterations) {
            let ptr = black_box(malloc(8192));
            black_box(free(ptr));
        }
        let med_time = start.elapsed();
        println!(
            "Medium (8KB): {:?} ({:.2} ns/op)",
            med_time,
            med_time.as_nanos() as f64 / iterations as f64
        );

        // Bench large allocations
        let start = Instant::now();
        for _ in black_box(0..10000) {
            let ptr = black_box(malloc(1024 * 1024 * 1));
            black_box(free(ptr));
        }

        let large_time = start.elapsed();
        println!(
            "Large (1MB): {:?} ({:.2} ns/op)",
            large_time,
            large_time.as_nanos() as f64 / 10000.0
        );
    }
}

#[test]
fn smoke_global_reuse() {
    unsafe {
        use crate::{abi::free::free, abi::malloc::malloc};
        use std::thread;

        // Fill caches in another thread and let it drop, so its freelist moves to the global pool.
        let worker = thread::spawn(|| {
            for _ in 0..10_000 {
                let ptr = malloc(128);
                free(ptr);
            }
        });
        worker.join().unwrap();

        // Main thread should be able to pull from the global list without crashing.
        for _ in 0..1000 {
            let ptr = malloc(128);
            assert!(!ptr.is_null());
            free(ptr);
        }
    }
}

#[test]
fn bootstrap_sets_va_len() {
    use crate::va::bootstrap::{VA_LEN, boot_strap};
    use std::sync::atomic::Ordering;

    unsafe { boot_strap() };
    assert!(VA_LEN.load(Ordering::Relaxed) > 0);
}

#[test]
fn realloc_handles_posix_memalign_pointer() {
    use crate::abi::{
        align::posix_memalign, free::free, malloc::malloc_usable_size, realloc::realloc,
    };

    unsafe {
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
        assert_eq!(posix_memalign(&mut ptr, 64, 100), 0);
        assert!(!ptr.is_null());
        assert_eq!((ptr as usize) % 64, 0);

        let initial = std::slice::from_raw_parts_mut(ptr as *mut u8, 100);
        for (i, byte) in initial.iter_mut().enumerate() {
            *byte = (i as u8).wrapping_mul(3).wrapping_add(1);
        }

        let old_usable = malloc_usable_size(ptr) as usize;
        assert!(old_usable >= 100);

        let new_ptr = realloc(ptr, old_usable + 32);
        assert!(!new_ptr.is_null());

        let after = std::slice::from_raw_parts(new_ptr as *const u8, 100);
        for i in 0..100 {
            assert_eq!(after[i], (i as u8).wrapping_mul(3).wrapping_add(1));
        }

        free(new_ptr);
    }
}
