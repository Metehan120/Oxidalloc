use rustix::io::Errno;
use std::{
    fmt::Debug,
    sync::{OnceLock, atomic::AtomicUsize},
    time::Instant,
};

// TODO: Add documentation to the entire codebase, will be added in ~3 days

pub mod big_allocation;
pub mod slab;
pub mod va;

pub const MAGIC: u64 = 0x01B01698BF0BEEF;
pub static OX_GLOBAL_STAMP: OnceLock<Instant> = OnceLock::new();
pub static OX_CURRENT_STAMP: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_IN_USE: AtomicUsize = AtomicUsize::new(0);

pub fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

pub const HEADER_SIZE: usize = size_of::<OxHeader>();

#[repr(C, align(16))]
pub struct OxHeader {
    next: *mut OxHeader,
    size: u64,
    magic: u64,
    flag: i32,
    life_time: usize,
    in_use: u8,
    thread_id: u32,
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
                self, ptr, extra, errno
            );
        } else {
            eprintln!("[OXIDALLOC FATAL] {:?} at ptr={:p} | {}", self, ptr, extra);
        }
        std::process::abort();
    }
}
