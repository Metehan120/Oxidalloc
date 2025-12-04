use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use libc::{__errno_location, MAP_FAILED, madvise};

use crate::{
    FLAG_NON, HEADER_SIZE, MAP, OxHeader, PROT, TOTAL_ALLOCATED, thread_local::ThreadLocalEngine,
};

pub static IS_BOOTSRAP: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

pub const MAGIC: u64 = 0x01B01698BF0BEEF;

pub const SIZE_CLASSES: [usize; 20] = [
    16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288,
    1048576, 2097152, 4194304, 8388608,
];

// Iterations of each size class, each iteration is a try to allocate a chunk of memory
pub const ITERATIONS: [usize; 20] = [
    512, // <=16B   - tons of tiny allocations (strings, small objects)
    512, // 32B   - very common (pointers, small structs)
    256, // 64B   - cache-line sized, super common
    256, // 128B  - still very frequent
    128, // 256B  - common
    64,  // 512B  - common
    32,  // 1KB   - moderate
    16,  // 2KB   - moderate
    32,  // 4KB   - page-sized, common for buffers
    8,   // 8KB   - still fairly common
    4,   // 16KB  - less common
    2,   // 32KB  - getting rare
    2,   // 64KB  - rare
    2,   // 128KB - rare
    2,   // 256KB - very rare
    1,   // 512KB - very rare
    1,   // 1MB   - almost never
    1,   // 2MB   - almost never
    1,   // 4MB   - almost never
    1,   // 8MB   - almost never
];

pub static VA_START: AtomicUsize = AtomicUsize::new(0);
pub static VA_END: AtomicUsize = AtomicUsize::new(0);
pub static VA_OFFSET: AtomicUsize = AtomicUsize::new(0);

pub fn bootstrap() {
    if IS_BOOTSRAP.load(Ordering::Relaxed) {
        return;
    }

    let _lock = GLOBAL_LOCK.lock().expect("[OXIDALLOC] Bootstrap failed");
    if IS_BOOTSRAP.load(Ordering::Relaxed) {
        return;
    }

    IS_BOOTSRAP.store(true, Ordering::Relaxed);

    init_va();

    ThreadLocalEngine::get_or_init();

    let random_start = init_va_offset();
    VA_OFFSET.store(random_start, Ordering::Release);
}

fn init_va() {
    unsafe {
        const SIZE: usize = 1024 * 1024 * 1024 * 256;

        let probe = libc::mmap(
            std::ptr::null_mut(),
            SIZE,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE,
            -1,
            0,
        );

        if probe == libc::MAP_FAILED {
            let err = *libc::__errno_location();
            panic!("VA init failed (errno = {})", err);
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
            let hint = VA_OFFSET.fetch_add(aligned_size, Ordering::Relaxed);

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

                prev = current_header;
            }

            let mut tail = prev;
            for _ in 0..ITERATIONS[class] - 1 {
                tail = (*tail).next;
            }

            let thread = ThreadLocalEngine::get_or_init();
            thread.push_to_thread_tailed(class, prev, tail);
            thread.usages[class].fetch_add(ITERATIONS[class], Ordering::Relaxed);

            TOTAL_ALLOCATED.fetch_add(total, Ordering::Relaxed);

            true
        }
    }
}
