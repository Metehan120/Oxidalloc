#![warn(clippy::nursery, clippy::pedantic)]
#![allow(
    static_mut_refs,
    unsafe_op_in_unsafe_fn,
    clippy::ptr_as_ptr,
    clippy::inline_always,
    clippy::new_without_default,
    clippy::ref_as_ptr,
    clippy::cast_ptr_alignment,
    clippy::module_name_repetitions,
    clippy::use_self
)]
#![feature(thread_local)]
#![feature(likely_unlikely)]

use std::{fmt::Debug, sync::atomic::AtomicUsize, time::Instant, usize};

use crate::internals::oncelock::OnceLock;

pub mod abi;
pub mod big_allocation;
pub mod internals;
pub mod slab;
pub mod sys;
pub mod trim;
pub mod va;

pub enum Err {
    OutOfReservation,
    OutOfMemory,
}

pub const MAX_INTERCONNECT_CACHE: usize = 4;
pub const MAX_NUMA_NODES: usize = 4; // Adapt this to the number of NUMA nodes in your system

pub const EROCMEF: i32 = 41; // harmless let it stay
pub const VERSION: u32 = 0xABA01;
pub const OX_ALIGN_TAG: usize = usize::from_le_bytes(*b"OXIDALGN");
pub const FLAG_ALIGNED: u8 = 2;

#[cfg(feature = "hardened-malloc")]
pub static mut MAGIC: u64 = 0x01B01698BF0BEEF;
#[cfg(feature = "hardened-malloc")]
pub static mut FREED_MAGIC: u64 = 0x12BE34FF09EBEAFF;

#[cfg(not(feature = "hardened-malloc"))]
pub static mut MAGIC: u8 = 0;
#[cfg(not(feature = "hardened-malloc"))]
pub static mut FREED_MAGIC: u8 = 1;

pub static OX_GLOBAL_STAMP: OnceLock<Instant> = OnceLock::new();
pub static mut OX_CURRENT_STAMP: u32 = 0;

pub static AVERAGE_BLOCK_TIMES_PTHREAD: AtomicUsize = AtomicUsize::new(3000);
pub static AVERAGE_BLOCK_TIMES_GLOBAL: AtomicUsize = AtomicUsize::new(3000);

pub static OX_TRIM_THRESHOLD: AtomicUsize = AtomicUsize::new(1024 * 1024 * 10);
pub static mut OX_FORCE_THP: bool = false;
pub static mut OX_TRIM: bool = false;
pub static OX_MAX_RESERVATION: AtomicUsize = AtomicUsize::new(1024 * 1024 * 1024 * 16);

pub fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

pub(crate) fn reset_fork_onces() {
    OX_GLOBAL_STAMP.reset_on_fork();
}

pub const HEADER_SIZE: usize = size_of::<OxHeader>();

#[repr(C, align(16))]
pub struct MetaData {
    pub start: usize,
    pub end: usize,
    pub next: usize,
}

#[derive(Debug, Clone)]
#[cfg(not(feature = "hardened-linked-list"))]
#[repr(C, align(16))]
pub struct OxHeader {
    pub next: *mut OxHeader,
    pub class: u8,
    pub magic: u8,
    pub life_time: u32,
}

#[derive(Debug, Clone)]
#[cfg(feature = "hardened-linked-list")]
#[repr(C, align(16))]
pub struct OxHeader {
    pub magic: u64,
    pub next: *mut OxHeader,
    pub class: u8,
    pub life_time: u32,
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
    ICCFailedToInitialize = 0x100C,
}

impl Debug for OxidallocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DoubleFree => write!(f, "DoubleFree (0x1000)"),
            Self::MemoryCorruption => write!(f, "MemoryCorruption (0x1001)"),
            Self::InvalidSize => write!(f, "InvalidSize (0x1002)"),
            Self::OutOfMemory => write!(f, "OutOfMemory (0x1003)"),
            Self::VaBitmapExhausted => write!(f, "VaBitmapExhausted (0x1004)"),
            Self::VAIinitFailed => write!(f, "VAIinitFailed (0x1005)"),
            Self::PThreadCacheFailed => write!(f, "PThreadCacheFailed (0x1006)"),
            Self::TooMuchQuarantine => write!(f, "TooMuchQuarantine (0x1007)"),
            Self::DoubleQuarantine => write!(f, "DoubleQuarantine (0x1008)"),
            Self::ReservationExceeded => write!(f, "ReservationExceeded (0x1009)"),
            Self::SecurityViolation => write!(f, "SecurityViolation (0x100A)"),
            Self::AttackOrCorruption => write!(f, "AttackOrCorruption (0x100B)"),
            Self::ICCFailedToInitialize => write!(f, "ICCFailedToInitialize (0x100C)"),
        }
    }
}

impl OxidallocError {
    pub fn log_and_abort(&self, ptr: *mut std::ffi::c_void, extra: &str, errno: Option<i32>) -> ! {
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
