use crate::OxHeader;

pub mod bulk_allocation;
pub mod global;
pub mod quarantine;
pub mod thread_local;

pub const SIZE_CLASSES: [usize; 36] = [
    16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320, 352, 384,
    416, 448, 480, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288,
    1048576, 2097152,
];

pub const NUM_SIZE_CLASSES: usize = SIZE_CLASSES.len();

// Iterations of each size class, each iteration is a try to allocate a chunk of memory
pub const ITERATIONS: [usize; 36] = [
    64, // 16B   - tons of tiny allocations (strings, small objects)
    64, // 32B   - very common (pointers, small structs)
    32, // 48B   - less common but still frequent
    64, // 64B   - cache-line sized, super common
    16, // 80B   - less common but still frequent
    32, // 96B   - less common but still frequent
    24, // 112B  - still very frequent
    32, // 128B  - common
    12, // 144B  - common
    12, // 160B  - common
    8,  // 176B  - common
    14, // 192B  - common
    8,  // 208B  - common
    8,  // 224B  - common
    4,  // 240B  - common
    12, // 256B  - common
    2,  // 288B  - common
    2,  // 320B  - common
    3,  // 352B  - common
    4,  // 384B  - common
    3,  // 416B  - common
    4,  // 448B  - common
    2,  // 480B  - common
    4,  // 512B  - common
    2,  // 1KB   - moderate
    2,  // 2KB   - moderate
    8,  // 4KB   - page-sized, common for buffers
    4,  // 8KB   - still fairly common
    2,  // 16KB  - less common
    1,  // 32KB  - getting rare
    1,  // 64KB  - rare
    1,  // 128KB - rare
    1,  // 256KB - very rare
    1,  // 512KB - very rare
    1,  // 1MB   - almost never
    1,  // 2MB   - almost never
];

pub fn match_size_class(size: usize) -> Option<usize> {
    for (i, &class_size) in SIZE_CLASSES.iter().enumerate() {
        if size <= class_size {
            return Some(i);
        }
    }
    None
}

#[inline(always)]
pub fn unpack_header(header: usize, random: usize) -> *mut OxHeader {
    (header ^ random) as *mut OxHeader
}

#[inline(always)]
pub fn pack_header(header: *mut OxHeader, random: usize) -> usize {
    (header as usize) ^ random
}
