pub mod bulk_allocation;
pub mod global;
pub mod quarantine;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 20] = [
    8, 16, 24, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144,
    524288, 1048576, 2097152,
];

pub const NUM_SIZE_CLASSES: usize = SIZE_CLASSES.len();

// Iterations of each size class, each iteration is a try to allocate a chunk of memory
pub const ITERATIONS: [usize; 20] = [
    2048, 1024, 1024, // <=16B   - tons of tiny allocations (strings, small objects)
    1024, // 32B   - very common (pointers, small structs)
    512,  // 64B   - cache-line sized, super common
    512,  // 128B  - still very frequent
    256,  // 256B  - common
    128,  // 512B  - common
    64,   // 1KB   - moderate
    32,   // 2KB   - moderate
    16,   // 4KB   - page-sized, common for buffers
    8,    // 8KB   - still fairly common
    4,    // 16KB  - less common
    2,    // 32KB  - getting rare
    2,    // 64KB  - rare
    1,    // 128KB - rare
    1,    // 256KB - very rare
    1,    // 512KB - very rare
    1,    // 1MB   - almost never
    1,    // 2MB   - almost never
];

pub fn match_size_class(size: usize) -> Option<usize> {
    for (i, &class_size) in SIZE_CLASSES.iter().enumerate() {
        if size <= class_size {
            return Some(i);
        }
    }
    None
}
