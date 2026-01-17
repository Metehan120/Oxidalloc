#![allow(unsafe_op_in_unsafe_fn)]
use std::{hint::unlikely, sync::OnceLock};

use crate::OxHeader;

pub mod bulk_allocation;
pub mod global;
pub mod quarantine;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 34] = [
    // Tiny (16-128) - 16 Byte steps
    16, 32, 48, 64, 80, 96, 128, // Small (160-512) - 32/64 Byte steps
    160, 192, 256, 320, 384, 512, // Medium (768-3072) - Large steps
    768, 1024, 1280, 1536, 1792, 2048, 2560, 3072, // Large (3840-24KB)
    3840, 4096, 8192, 12288, 16384, 24576, // Very Large (32KB+)
    32768, 65536, 131072, 262144, 524288, 1048576, 2097152,
];

pub const ITERATIONS: [usize; 34] = [
    // TINY (Targets ~64KB / 16 Pages)
    // 16B  -> Block 80B  -> 65536 / 80  = 819 (0 Waste)
    // 32B  -> Block 96B  -> 65536 / 96  = 682 (64B Waste)
    819, 682, 585, 511, 455, 307, 341,
    // --- SMALL (Targets ~16KB / 4 Pages)
    // 160B -> Block 224B -> 16384 / 224 = 73 (16B Waste)
    73, 63, 51, 42, 36, 28,
    // MEDIUM (Targets ~16KB or ~8KB)
    // 768B -> Block 832B -> 16384 / 832 = 19 (560B Waste)
    // We drop to N=1 quickly for sizes > 2KB to prevent VIRT bloat
    19, 15, 12, 5, 8, 7, 3, 5,
    // LARGE (Targets 1 Block)
    // For sizes > 3KB, we want Malloc/Free to be 1:1 with mmap/munmap logic
    // via bulk_fill to allow immediate reclamation by gtrim and ptrim
    1, 1, 1, 1, 1, 1,
    // --- VERY LARGE ---
    //
    // Always 1. Let the OS handle the pages.
    1, 1, 1, 1, 1, 1, 1,
];

static SIZE_LUT: [u8; 64] = {
    let mut lut = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        let size = (i + 1) * 16;
        let mut class = 0;
        while class < 34 && SIZE_CLASSES[class] < size {
            class += 1;
        }
        lut[i] = class as u8;
        i += 1;
    }
    lut
};

pub const NUM_SIZE_CLASSES: usize = SIZE_CLASSES.len();
static CLASS_4096: OnceLock<usize> = OnceLock::new();

pub fn get_size_4096_class() -> usize {
    *CLASS_4096.get_or_init(|| SIZE_CLASSES.iter().position(|&s| s >= 4096).unwrap())
}

#[inline(always)]
pub fn match_size_class(size: usize) -> Option<usize> {
    if unlikely(size == 0 || size > 2097152) {
        return None;
    }

    if size <= 1024 {
        let index = (size - 1) >> 4;
        return Some(unsafe { *SIZE_LUT.get_unchecked(index) as usize });
    }

    slow_path_match(size)
}

#[inline(always)]
fn slow_path_match(size: usize) -> Option<usize> {
    for i in 15..NUM_SIZE_CLASSES {
        if size <= SIZE_CLASSES[i] {
            return Some(i);
        }
    }
    None
}

#[inline(always)]
pub unsafe fn xor_ptr_general(ptr: *mut OxHeader, _key: usize) -> *mut OxHeader {
    #[cfg(feature = "hardened")]
    {
        if unlikely(ptr.is_null()) {
            return std::ptr::null_mut();
        }
        ((ptr as usize) ^ _key) as *mut OxHeader
    }

    #[cfg(not(feature = "hardened"))]
    {
        ptr
    }
}

#[inline(always)]
pub unsafe fn xor_ptr_numa(ptr: *mut OxHeader, _numa: usize) -> *mut OxHeader {
    #[cfg(feature = "hardened")]
    {
        use crate::va::bootstrap::PER_NUMA_KEY;

        if unlikely(ptr.is_null()) {
            return std::ptr::null_mut();
        }
        ((ptr as usize) ^ PER_NUMA_KEY[_numa]) as *mut OxHeader
    }

    #[cfg(not(feature = "hardened"))]
    {
        ptr
    }
}
