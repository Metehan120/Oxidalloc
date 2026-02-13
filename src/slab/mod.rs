use std::hint::unlikely;

use crate::{HEADER_SIZE, OxHeader, internals::oncelock::OnceLock, va::align_to};

pub mod bulk_allocation;
pub mod interconnect;
pub mod rseq_general;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 18] = [
    16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288,
    1048576, 2097152,
];

pub const ITERATIONS: [usize; 18] = [
    2048, 1024, 512, 256, // 16-128 (Targets ~32KB)
    32, 16, 8, 4, 2, // 256-4096 (Targets ~8KB)
    1, 1, 1, 1, 1, 1, 1, 1, 1, // 8KB+ (1 Block)
];

const TLS_MAX_SIZE_CLASS: usize = 1024 * 256;
const TLS_BIG_CLASS_BYTES: usize = 1024 * 64;
const TLS_MEDIUM_CLASS_BYTES: usize = 1024 * 96;
const TLS_SMALL_CLASS_BYTES: usize = 1024 * 128;
pub const TLS_MAX_BLOCKS: [usize; NUM_SIZE_CLASSES] = {
    let mut arr = [0; NUM_SIZE_CLASSES];
    let mut i = 0;

    while i < NUM_SIZE_CLASSES {
        let payload = SIZE_CLASSES[i];
        let block_size = align_to(payload + HEADER_SIZE, 16);
        let mut blocks = if block_size > TLS_MAX_SIZE_CLASS {
            0
        } else {
            if payload < 256 {
                TLS_SMALL_CLASS_BYTES / block_size
            } else if payload < 1024 * 16 {
                TLS_MEDIUM_CLASS_BYTES / block_size
            } else {
                TLS_BIG_CLASS_BYTES / block_size
            }
        };

        if blocks == 0 {
            blocks = 1;
        }

        arr[i] = blocks;
        i += 1;
    }

    arr
};

pub static SIZE_LUT: [u8; 256] = {
    let mut lut = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let size = (i + 1) * 16;
        let mut class = 0;
        while class < 18 && SIZE_CLASSES[class] < size {
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

#[cfg(not(feature = "global-alloc"))]
pub(crate) fn reset_fork_onces() {
    CLASS_4096.reset_on_fork();
}

#[inline(always)]
pub fn match_size_class(size: usize) -> Option<usize> {
    if unlikely(size == 0 || size > 2097152) {
        return None;
    }

    if unlikely(size <= 4096 && size > 0) {
        let index = (size - 1) >> 4;
        return Some(unsafe { *SIZE_LUT.get_unchecked(index) as usize });
    }

    slow_path_match(size)
}

#[inline(always)]
fn slow_path_match(size: usize) -> Option<usize> {
    for i in 0..NUM_SIZE_CLASSES {
        if size <= SIZE_CLASSES[i] {
            return Some(i);
        }
    }
    None
}

#[inline(always)]
pub unsafe fn xor_ptr_general(ptr: *mut OxHeader, _key: usize) -> *mut OxHeader {
    #[cfg(feature = "hardened-linked-list")]
    {
        if unlikely(ptr.is_null()) {
            return std::ptr::null_mut();
        }
        ((ptr as usize) ^ _key) as *mut OxHeader
    }

    #[cfg(not(feature = "hardened-linked-list"))]
    {
        ptr
    }
}
