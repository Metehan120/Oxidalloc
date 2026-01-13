use std::sync::OnceLock;

pub mod bulk_allocation;
pub mod global;
pub mod quarantine;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 34] = [
    16, 32, 48, 64, 80, 96, 128, 160, 192, 256, 320, 384, 512, 768, 1024, 1280, 1536, 1792, 2048,
    3072, 3840, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 65536, 131072, 262144, 524288,
    1048576, 2097152,
];

pub const NUM_SIZE_CLASSES: usize = SIZE_CLASSES.len();
const CLASS_4096: OnceLock<usize> = OnceLock::new();

pub fn get_size_4096_class() -> usize {
    *CLASS_4096.get_or_init(|| SIZE_CLASSES.iter().position(|&s| s >= 4096).unwrap())
}

pub const ITERATIONS: [usize; 34] = [
    // Tiny (16B-128B)
    512, 256, 128, 128, 64, 64, 64, // Small (160B-512B)
    32, 32, 32, 16, 16, 16, // Medium (768B-2KB)
    8, 8, 4, 4, 4, 4, 4, 4, // Large (3KB-16KB)
    2, 2, 2, 1, 1, 1, // Very Large (32KB-256KB)
    1, 1, 1, 1, 1, 1, 1,
];

#[inline(always)]
pub fn match_size_class(size: usize) -> Option<usize> {
    for (i, &class_size) in SIZE_CLASSES.iter().enumerate() {
        if size <= class_size {
            return Some(i);
        }
    }
    None
}
