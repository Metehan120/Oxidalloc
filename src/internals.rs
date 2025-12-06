use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use libc::{__errno_location, MAP_FAILED, madvise};

use crate::{
    DEFAULT_TRIM_INTERVAL, FLAG_NON, GLOBAL_TRIM_INTERVAL, HEADER_SIZE, LOCAL_TRIM_INTERVAL, MAP,
    OxHeader, OxidallocError, PROT, thread_local::ThreadLocalEngine,
};

pub static IS_BOOTSRAP: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

pub const MAGIC: u64 = 0x01B01698BF0BEEF;

pub const SIZE_CLASSES: [usize; 22] = [
    8, 16, 24, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144,
    524288, 1048576, 2097152, 4194304, 8388608,
];

// Iterations of each size class, each iteration is a try to allocate a chunk of memory
pub const ITERATIONS: [usize; 22] = [
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
    2,    // 128KB - rare
    2,    // 256KB - very rare
    1,    // 512KB - very rare
    1,    // 1MB   - almost never
    1,    // 2MB   - almost never
    1,    // 4MB   - almost never
    1,    // 8MB   - almost never
];

pub static VA_START: AtomicUsize = AtomicUsize::new(0);
pub static VA_END: AtomicUsize = AtomicUsize::new(0);
pub static VA_OFFSET: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
pub fn bootstrap() {
    if IS_BOOTSRAP.load(Ordering::Relaxed) {
        return;
    }

    let _lock = GLOBAL_LOCK.lock().expect("[OXIDALLOC] Bootstrap failed");
    if IS_BOOTSRAP.load(Ordering::Relaxed) {
        return;
    }

    IS_BOOTSRAP.store(true, Ordering::Relaxed);

    unsafe {
        let key = b"OXIDALLOC_TRIM_INTERVAL\0";
        let value_ptr = libc::getenv(key.as_ptr() as *const i8);

        if !value_ptr.is_null() {
            let mut val = 0usize;
            let mut ptr = value_ptr as *const u8;

            while *ptr != 0 {
                if *ptr >= b'0' && *ptr <= b'9' {
                    val = val * 10 + (*ptr - b'0') as usize;
                } else {
                    break;
                }
                ptr = ptr.add(1);
            }

            if val == 0 || val < 100 {
                val = DEFAULT_TRIM_INTERVAL;
            }

            GLOBAL_TRIM_INTERVAL.store(val, Ordering::Relaxed);
            LOCAL_TRIM_INTERVAL.store(val / 2, Ordering::Relaxed);
        } else {
            let val = DEFAULT_TRIM_INTERVAL;
            GLOBAL_TRIM_INTERVAL.store(val, Ordering::Relaxed);
            LOCAL_TRIM_INTERVAL.store(val / 2, Ordering::Relaxed);
        }
    }

    init_va();
    ThreadLocalEngine::get_or_init();
    let random_start = init_va_offset();
    VA_OFFSET.store(random_start, Ordering::Release);
}

fn init_va() {
    unsafe {
        const SIZE: usize = 1024 * 1024 * 1024 * 512;

        let probe = libc::mmap(
            std::ptr::null_mut(),
            SIZE,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE,
            -1,
            0,
        );

        if probe == libc::MAP_FAILED {
            OxidallocError::VAIinitFailed
                .log_and_abort(probe, "Kernel does not handing virtual memory");
        }

        let start = probe as usize;
        let end = start + SIZE;

        VA_START.store(start, Ordering::Relaxed);
        VA_END.store(end, Ordering::Relaxed);
        VA_OFFSET.store(start, Ordering::Relaxed);
    }
}

fn init_va_offset() -> usize {
    let start = VA_START.load(Ordering::Relaxed);
    let end = VA_END.load(Ordering::Relaxed);

    let mut rng = 0u64;
    unsafe {
        libc::getrandom(&mut rng as *mut u64 as *mut c_void, 8, 0);
    }

    let offset = (rng as usize) % (end - start);
    let aligned_offset = offset & !4095;

    start + aligned_offset
}

pub fn align(size: usize) -> usize {
    (size + 4095) & !4095
}

pub struct AllocationHelper;

impl AllocationHelper {
    pub fn match_size_class(&self, size: usize) -> Option<usize> {
        for (i, &class_size) in SIZE_CLASSES.iter().enumerate() {
            if size <= class_size {
                return Some(i);
            }
        }
        None
    }

    pub fn bulk_allocate(&self, class: usize) -> bool {
        unsafe {
            let size = SIZE_CLASSES[class] + HEADER_SIZE;
            let count = ITERATIONS[class];
            let total = (size * count) + 4096;

            let aligned_size = align(total);
            let hint = match VA_MAP.alloc(aligned_size) {
                Some(size) => size,
                None => {
                    eprintln!("[LIBOXIDALLOC] Failed to allocate virtual address space");
                    return false;
                }
            };

            let chunk = libc::mmap(
                hint as *mut c_void,
                aligned_size,
                PROT,
                MAP | libc::MAP_FIXED,
                -1,
                0,
            );

            if chunk == MAP_FAILED {
                eprintln!(
                    "[LIBOXIDALLOC] Something went wrong during allocation, errno: {:?}",
                    *__errno_location()
                );
                return false;
            }

            if class > 14 {
                madvise(chunk, total, libc::MADV_HUGEPAGE);
            }

            let mut prev = null_mut();

            for i in (0..ITERATIONS[class]).rev() {
                let current_header = (chunk as usize + i * size) as *mut OxHeader;
                (*current_header).next = prev;
                (*current_header).size = SIZE_CLASSES[class] as u64;
                (*current_header).magic = 0;
                (*current_header).flag = FLAG_NON;
                (*current_header).in_use.store(0, Ordering::Relaxed);

                prev = current_header;
            }

            let mut tail = prev;
            for _ in 0..ITERATIONS[class] - 1 {
                tail = (*tail).next;
            }

            let thread = ThreadLocalEngine::get_or_init();
            thread.push_to_thread_tailed(class, prev, tail);
            thread.usages[class].fetch_add(ITERATIONS[class], Ordering::Relaxed);

            true
        }
    }
}

pub const BLOCK_SIZE: usize = 64 * 1024;
pub const BITMAP_LEN: usize = 65_536;

pub static VA_MAP: VaBitmap = VaBitmap::new();

// Half of written by AI, I was not able to complete it at the time so there should be many ways to improve it.
// TODO: Catch errors and optimize it
// - Metehan
pub struct VaBitmap {
    map: [AtomicU64; BITMAP_LEN],
    hint: AtomicUsize,
}

impl VaBitmap {
    pub const fn new() -> Self {
        Self {
            map: [const { AtomicU64::new(0) }; BITMAP_LEN],
            hint: AtomicUsize::new(0),
        }
    }

    pub fn alloc(&self, size: usize) -> Option<usize> {
        let needed = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;

        if needed == 1 {
            return self.alloc_single();
        }
        self.alloc_multi(needed)
    }

    fn alloc_single(&self) -> Option<usize> {
        let start = self.hint.load(Ordering::Relaxed);
        for i in start..BITMAP_LEN {
            let chunk = self.map[i].load(Ordering::Relaxed);
            if chunk == u64::MAX {
                continue;
            }

            let bit = (!chunk).trailing_zeros();
            let mask = 1u64 << bit;

            if (self.map[i].fetch_or(mask, Ordering::Acquire) & mask) == 0 {
                self.hint.store(i, Ordering::Relaxed);
                let global_idx = (i * 64) + bit as usize;
                let addr = VA_START.load(Ordering::Relaxed) + (global_idx * BLOCK_SIZE);
                return Some(addr);
            }
        }
        None
    }

    fn alloc_multi(&self, count: usize) -> Option<usize> {
        let total_bits = BITMAP_LEN * 64;

        if count > total_bits {
            return None;
        }

        let start_bit = self.hint.load(Ordering::Relaxed) * 64;
        let mut current_run = 0;
        let mut run_start = 0;

        for global_bit in start_bit..total_bits {
            let chunk_idx = global_bit / 64;
            let bit_in_chunk = global_bit % 64;

            let chunk = self.map[chunk_idx].load(Ordering::Relaxed);

            if (chunk & (1u64 << bit_in_chunk)) != 0 {
                current_run = 0;
            } else {
                if current_run == 0 {
                    run_start = global_bit;
                }
                current_run += 1;

                if current_run == count {
                    if self.try_claim(run_start, count) {
                        self.hint.store(chunk_idx, Ordering::Relaxed);
                        let addr = VA_START.load(Ordering::Relaxed) + (run_start * BLOCK_SIZE);
                        return Some(addr);
                    }
                    current_run = 0;
                }
            }
        }
        None
    }

    fn try_claim(&self, start_idx: usize, count: usize) -> bool {
        let total_bits = BITMAP_LEN * 64;

        if start_idx >= total_bits || start_idx + count > total_bits {
            return false;
        }

        for k in 0..count {
            let idx = start_idx + k;
            let chunk_idx = idx / 64;

            if chunk_idx >= BITMAP_LEN {
                self.rollback(start_idx, k);
                return false;
            }

            let mask = 1u64 << (idx % 64);

            let prev = self.map[chunk_idx].fetch_or(mask, Ordering::Acquire);

            if (prev & mask) != 0 {
                self.rollback(start_idx, k);
                return false;
            }
        }
        true
    }

    fn rollback(&self, start_idx: usize, count: usize) {
        for k in 0..count {
            let idx = start_idx + k;
            let chunk_idx = idx / 64;

            if chunk_idx >= BITMAP_LEN {
                return;
            }

            let mask = 1u64 << (idx % 64);
            self.map[chunk_idx].fetch_and(!mask, Ordering::Release);
        }
    }

    pub fn free(&self, addr: usize, size: usize) {
        let start = VA_START.load(Ordering::Relaxed);
        if addr < start {
            return;
        }

        let offset = addr - start;
        let start_idx = offset / BLOCK_SIZE;
        let count = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;

        self.rollback(start_idx, count);

        let chunk = start_idx / 64;
        if chunk < self.hint.load(Ordering::Relaxed) {
            self.hint.store(chunk, Ordering::Relaxed);
        }
    }
}
