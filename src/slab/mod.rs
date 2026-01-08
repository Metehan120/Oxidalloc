pub mod bulk_allocation;
pub mod global;
pub mod quarantine;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 31] = [
    16, 24, 32, 48, 64, 80, 96, 128, 160, 192, 256, 320, 384, 512, 768, 1024, 1280, 1536, 1792,
    2048, 3072, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576, 2097152,
];

pub const NUM_SIZE_CLASSES: usize = SIZE_CLASSES.len();

pub const ITERATIONS: [usize; 31] = [
    // Tiny (16B-128B) - super common, allocate tons
    1024, 512, 512, 256, 256, 128, 128, 128,
    // Small (160B-512B) - common, moderate batches
    64, 64, 64, 32, 32, 32, // Medium (768B-2KB) - moderate frequency
    16, 16, 8, 8, 8, 8, // Large (3KB-16KB) - less common
    4, 4, 2, 2, // Very Large (32KB-256KB) - rare
    1, 1, 1, 1, // Huge (512KB-2MB) - very rare
    1, 1, 1,
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
