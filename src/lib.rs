#![warn(clippy::nursery, clippy::pedantic)]

use rustix::{
    io::Errno,
    mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous},
};
use std::{
    fmt::Debug,
    hint::spin_loop,
    os::raw::c_void,
    ptr::{null_mut, write_bytes},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

use crate::{
    slab::{
        ITERATIONS,
        global::{GLOBAL, GLOBAL_LOCKS, GLOBAL_USAGE},
        thread_local::THREAD_REGISTER,
    },
    va::{bitmap::VA_MAP, va_helper::is_ours},
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
pub static OX_TRIM_THRESHOLD: AtomicUsize = AtomicUsize::new(1024 * 1024 * 10);
pub static OX_USE_THP: AtomicBool = AtomicBool::new(false);
pub static OX_DEBUG: AtomicBool = AtomicBool::new(false);

pub fn get_clock() -> &'static Instant {
    OX_GLOBAL_STAMP.get_or_init(|| Instant::now())
}

pub const HEADER_SIZE: usize = size_of::<OxHeader>();

#[repr(C, align(16))]
pub struct SlabMetadata {
    pub size: usize,
    pub ref_count: AtomicUsize,
}

#[repr(C, align(16))]
pub struct OxHeader {
    pub next: *mut OxHeader,
    pub size: u64,
    pub magic: u64,
    pub flag: i32,
    pub life_time: usize,
    pub in_use: u8,
    pub metadata: *mut SlabMetadata,
}

#[allow(unsafe_op_in_unsafe_fn)]
#[inline]
pub unsafe fn release_slab(metadata: *mut SlabMetadata, class: usize) -> bool {
    if metadata.is_null() || class >= GLOBAL.len() {
        return false;
    }

    let mut node = THREAD_REGISTER.load(Ordering::Acquire);
    let mut active = 0usize;
    while !node.is_null() {
        let engine = (*node).engine.load(Ordering::Acquire);
        if !engine.is_null() {
            active += 1;
            if active > 1 {
                return false;
            }
        }
        node = (*node).next.load(Ordering::Acquire);
    }

    let cvoid = metadata as *mut c_void;
    let in_use = (*metadata).ref_count.load(Ordering::Acquire);

    if in_use == 0 {
        release_blocks(metadata, class);
        let size = (*metadata).size;
        if mmap_anonymous(
            cvoid,
            size,
            ProtFlags::empty(),
            MapFlags::PRIVATE | MapFlags::FIXED | MapFlags::NORESERVE,
        )
        .is_err()
        {
            let is_failed = madvise(cvoid, size, Advice::LinuxDontNeed);
            if is_failed.is_err() {
                write_bytes(cvoid as *mut u8, 0, size);
            }
        }
        VA_MAP.free(metadata as usize, size);
        return true;
    }
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn prune_list_for_metadata(
    mut head: *mut OxHeader,
    metadata: *mut SlabMetadata,
) -> (*mut OxHeader, usize, usize) {
    let mut new_head: *mut OxHeader = null_mut();
    let mut new_tail: *mut OxHeader = null_mut();
    let mut kept = 0;
    let mut removed = 0;

    while !head.is_null() && is_ours(head as usize) {
        let mut next = (*head).next;
        if !next.is_null() && !is_ours(next as usize) {
            next = null_mut();
        }

        if (*head).metadata == metadata {
            removed += 1;
        } else {
            if new_head.is_null() {
                new_head = head;
            } else {
                (*new_tail).next = head;
            }
            new_tail = head;
            kept += 1;
        }

        if next.is_null() {
            break;
        }
        head = next;
    }

    if !new_tail.is_null() {
        (*new_tail).next = null_mut();
    }

    (new_head, kept, removed)
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn release_blocks(metadata: *mut SlabMetadata, class: usize) {
    if metadata.is_null() || class >= ITERATIONS.len() {
        return;
    }

    while GLOBAL_LOCKS[class]
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }

    let global_head = GLOBAL[class].load(Ordering::Relaxed);
    let (global_head, global_kept, _global_removed) =
        prune_list_for_metadata(global_head, metadata);

    GLOBAL[class].store(global_head, Ordering::Relaxed);
    GLOBAL_USAGE[class].store(global_kept, Ordering::Relaxed);
    GLOBAL_LOCKS[class].store(false, Ordering::Release);

    let mut node = THREAD_REGISTER.load(Ordering::Acquire);
    while !node.is_null() {
        let engine = (*node).engine.load(Ordering::Acquire);
        if !engine.is_null() {
            (*engine).lock(class);
            let local_head = (*engine).cache[class].load(Ordering::Relaxed);
            if engine.is_null() {
                continue;
            }

            let (local_head, local_kept, _local_removed) =
                prune_list_for_metadata(local_head, metadata);
            if engine.is_null() {
                continue;
            }

            (*engine).cache[class].store(local_head, Ordering::Relaxed);
            (*engine).usages[class].store(local_kept, Ordering::Relaxed);
            (*engine).latest_usages[class].store(local_kept, Ordering::Relaxed);

            if engine.is_null() {
                continue;
            }
            (*engine).unlock(class);
        }

        node = (*node).next.load(Ordering::Acquire);
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
