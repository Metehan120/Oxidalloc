use std::{
    fmt::Debug,
    os::raw::c_void,
    sync::{
        OnceLock,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Instant,
};

pub const PROT: libc::c_int = libc::PROT_READ | libc::PROT_WRITE;
pub const MAP: libc::c_int = libc::MAP_ANONYMOUS | libc::MAP_PRIVATE;

pub mod align;
pub mod calloc;
pub mod free;
pub mod global;
pub mod internals;
pub mod malloc;
pub mod realloc;
pub mod thread_local;
pub mod trim;

pub const HEADER_SIZE: usize = size_of::<OxHeader>();
pub const FLAG_NON: i32 = 0;
pub const FLAG_FREED: i32 = 2;
pub const FLAG_ALIGNED: i32 = 4;
pub const DEFAULT_TRIM_INTERVAL: usize = 20000;

pub static OX_GLOBAL_STAMP: OnceLock<Instant> = OnceLock::new();
pub static OX_CURRENT_STAMP: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_OPS: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_IN_USE: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_TRIM_INTERVAL: AtomicUsize = AtomicUsize::new(0);
pub static LOCAL_TRIM_INTERVAL: AtomicUsize = AtomicUsize::new(0);

fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

#[repr(C, align(16))]
pub struct OxHeader {
    next: *mut OxHeader,
    size: u64,
    magic: u64,
    flag: i32,
    life_time: usize,
    in_use: AtomicU8,
}

impl OxHeader {
    fn try_change_in_use(&self, expected: u8, new_state: u8) -> Result<u8, u8> {
        self.in_use
            .compare_exchange(expected, new_state, Ordering::AcqRel, Ordering::Acquire)
    }

    fn change_in_use_state(&mut self, ptr: *mut c_void) {
        match self.try_change_in_use(1, 0) {
            Ok(1) => {
                self.magic = 0;
            }
            Ok(_) => {
                self.magic = 0;
            }
            Err(0) => {
                OxidallocError::DoubleFree.log_and_abort(ptr, "DoubleFree");
            }
            Err(_) => {
                OxidallocError::MemoryCorruption.log_and_abort(ptr, "MemoryCorruption");
            }
        }
    }
}

#[repr(u32)]
pub enum OxidallocError {
    DoubleFree = 0x1000,
    MemoryCorruption = 0x1001,
    InvalidSize = 0x1002,
    OutOfMemory = 0x1003,
    VaBitmapExhausted = 0x1004,
    VAIinitFailed = 0x1005,
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
        }
    }
}

impl OxidallocError {
    pub fn log_and_abort(&self, ptr: *mut std::ffi::c_void, extra: &str) -> ! {
        eprintln!("[OXIDALLOC FATAL] {:?} at ptr={:p} | {}", self, ptr, extra);
        std::process::abort();
    }
}

#[test]
fn bench_allocator() {
    use crate::{free::free, malloc::malloc};
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
