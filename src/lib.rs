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

#[cfg(feature = "global-alloc")]
pub use crate::inner::global_alloc::Oxidalloc;
use crate::internals::oncelock::OnceLock;
use std::{fmt::Debug, sync::atomic::AtomicUsize, time::Instant, usize};

#[cfg(not(feature = "global-alloc"))]
pub(crate) mod abi;
pub(crate) mod big_allocation;
pub(crate) mod inner;
pub(crate) mod internals;
pub(crate) mod slab;
pub(crate) mod sys;
pub(crate) mod trim;
pub(crate) mod va;

pub(crate) enum Err {
    OutOfMemory,
}

pub const MAX_INTERCONNECT_CACHE: usize = 4;
pub const MAX_NUMA_NODES: usize = 32; // Adapt this to the number of NUMA nodes in your system
pub const VERSION_STR: &str = "1.0.0-alpha-2";

pub(crate) const OX_ALIGN_TAG: usize = usize::from_le_bytes(*b"OXIDALGN");

#[cfg(feature = "hardened-malloc")]
pub(crate) static mut MAGIC: u64 = 0x01B01698BF0BEEF;
#[cfg(feature = "hardened-malloc")]
pub(crate) static mut FREED_MAGIC: u64 = 0x12BE34FF09EBEAFF;

#[cfg(not(feature = "hardened-malloc"))]
pub(crate) static mut MAGIC: u8 = 0;
#[cfg(not(feature = "hardened-malloc"))]
pub(crate) static mut FREED_MAGIC: u8 = 1;

pub(crate) static OX_GLOBAL_STAMP: OnceLock<Instant> = OnceLock::new();
pub(crate) static mut OX_CURRENT_STAMP: u32 = 0;

pub(crate) static AVERAGE_BLOCK_TIMES_GLOBAL: AtomicUsize = AtomicUsize::new(3);
pub(crate) static OX_TRIM_THRESHOLD: AtomicUsize = AtomicUsize::new(1024 * 1024 * 10);
pub(crate) static OX_MAX_RESERVATION: AtomicUsize = AtomicUsize::new(1024 * 1024 * 1024 * 16);

pub(crate) static mut OX_DISABLE_THP: bool = false;
pub(crate) static mut OX_FORCE_THP: bool = false;
pub(crate) static mut OX_TRIM: bool = true;

pub(crate) fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

#[cfg(not(feature = "global-alloc"))]
pub(crate) fn reset_fork_onces() {
    OX_GLOBAL_STAMP.reset_on_fork();
}

pub(crate) const HEADER_SIZE: usize = size_of::<OxHeader>();

#[repr(C, align(16))]
pub(crate) struct MetaData {
    pub start: usize,
    pub end: usize,
    pub next: usize,
}

#[derive(Debug, Clone)]
#[cfg(not(feature = "hardened-linked-list"))]
#[repr(C, align(16))]
pub(crate) struct OxHeader {
    pub next: *mut OxHeader,
    pub class: u8,
    pub magic: u8,
    pub life_time: u32,
}

#[derive(Debug, Clone)]
#[cfg(feature = "hardened-linked-list")]
#[repr(C, align(16))]
pub(crate) struct OxHeader {
    pub magic: u64,
    pub next: *mut OxHeader,
    pub class: u8,
    pub life_time: u32,
}

#[repr(u32)]
pub(crate) enum OxidallocError {
    DoubleFree = 0x1000,
    MemoryCorruption = 0x1001,
    OutOfMemory = 0x1003,
    VaBitmapExhausted = 0x1004,
    VAIinitFailed = 0x1005,
    PThreadCacheFailed = 0x1006,
    SecurityViolation = 0x100A,
    AttackOrCorruption = 0x100B,
    ICCFailedToInitialize = 0x100C,
}

impl Debug for OxidallocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DoubleFree => write!(f, "DoubleFree (0x1000)"),
            Self::MemoryCorruption => write!(f, "MemoryCorruption (0x1001)"),
            Self::OutOfMemory => write!(f, "OutOfMemory (0x1003)"),
            Self::VaBitmapExhausted => write!(f, "VaBitmapExhausted (0x1004)"),
            Self::VAIinitFailed => write!(f, "VAIinitFailed (0x1005)"),
            Self::PThreadCacheFailed => write!(f, "PThreadCacheFailed (0x1006)"),
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
