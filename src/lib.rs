#![warn(clippy::nursery, clippy::pedantic)]
#![allow(static_mut_refs)]
#![feature(thread_local)]
#![feature(likely_unlikely)]
#![feature(native_link_modifiers_as_needed)]

use rustix::io::Errno;
use std::{
    fmt::Debug,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicUsize},
    },
    time::Instant,
    usize,
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

pub static MAX_NUMA_NODES: usize = 4; // Adapt this to the number of NUMA nodes in your system

pub const EROCMEF: i32 = 41; // harmless let it stay
pub const VERSION: u32 = 0xABA01;
pub const OX_ALIGN_TAG: usize = u64::from_le_bytes(*b"OXIDALGN") as usize;
pub static mut MAGIC: u64 = 0x01B01698BF0BEEF;
pub static mut FREED_MAGIC: u64 = 0x12BE34FF09EBEAFF;
pub static OX_GLOBAL_STAMP: OnceLock<Instant> = OnceLock::new();
pub static OX_CURRENT_STAMP: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_IN_USE: AtomicUsize = AtomicUsize::new(0);
pub static AVERAGE_BLOCK_TIMES_PTHREAD: AtomicUsize = AtomicUsize::new(3000);
pub static AVERAGE_BLOCK_TIMES_GLOBAL: AtomicUsize = AtomicUsize::new(3000);
pub static OX_TRIM_THRESHOLD: AtomicUsize = AtomicUsize::new(1024 * 1024 * 10);
pub static OX_USE_THP: AtomicBool = AtomicBool::new(false);
pub static OX_MAX_RESERVATION: AtomicUsize = AtomicUsize::new(1024 * 1024 * 1024 * 16);

pub fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

pub const HEADER_SIZE: usize = size_of::<OxHeader>();

#[repr(C, align(16))]
pub struct MetaData {
    pub start: usize,
    pub end: usize,
    pub next: usize,
}

#[cfg(not(feature = "hardened-linked-list"))]
#[repr(C, align(16))]
pub struct OxHeader {
    pub next: *mut OxHeader,
    pub size: usize,
    pub magic: u64,
    pub in_use: u8,
    pub life_time: usize,
    pub metadata: *mut MetaData,
}

#[cfg(feature = "hardened-linked-list")]
#[repr(C, align(16))]
pub struct OxHeader {
    pub magic: u64,
    pub size: usize,
    pub life_time: usize,
    pub next: *mut OxHeader,
    pub in_use: u8,
    pub metadata: *mut MetaData,
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
    ReservationExceeded = 0x1009,
    SecurityViolation = 0x100A,
    AttackOrCorruption = 0x100B,
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
            OxidallocError::ReservationExceeded => write!(f, "ReservationExceeded (0x1009)"),
            OxidallocError::SecurityViolation => write!(f, "SecurityViolation (0x100A)"),
            OxidallocError::AttackOrCorruption => write!(f, "AttackOrCorruption (0x100B)"),
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
