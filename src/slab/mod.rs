use std::sync::OnceLock;

pub mod bulk_allocation;
pub mod global;
pub mod quarantine;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 34] = [
    16, 32, 48, 64, 80, 96, 128, 160, 192, 256, 320, 384, 512, 768, 1024, 1280, 1536, 1792, 2048,
    2560, 3072, 3840, 4096, 8192, 12288, 16384, 24576, 32768, 65536, 131072, 262144, 524288,
    1048576, 2097152,
];

pub const ITERATIONS: [usize; 34] = [
    // Tiny (16B-128B) - Targets ~64KB (16 Pages) perfectly
    819, 597, 585, 511, 455, 307, 341, // Small (160B-512B) - Targets ~16KB (4 Pages)
    73, 63, 51, 42, 36, 28, // Medium (768B-2KB) - Targets ~16KB or ~8KB tightly
    19, 15, 6, 5, 8, 7, 3, 5, // Large (3KB-24KB) - Targets 4 Pages or 1 Block
    4, 3, 1, 1, 1, 1, // Very Large (32KB+)
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
    if size > 0 && size <= 1024 {
        let index = (size - 1) >> 4;
        return Some(SIZE_LUT[index] as usize);
    }

    for i in 15..NUM_SIZE_CLASSES {
        if size <= SIZE_CLASSES[i] {
            return Some(i);
        }
    }

    None
}
