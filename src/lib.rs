#![feature(thread_local)]

use libc::{MADV_DONTNEED, MADV_HUGEPAGE, madvise, munmap, size_t};
use std::{
    env,
    os::raw::c_void,
    sync::{
        LazyLock,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
};

const MAGIC: u64 = 0x01B01698BF;

pub const SIZE_CLASSES: [usize; 16] = [
    512, 4096, 6144, 8192, 16384, 32768, 65536, 262144, 1048576, 2097152, 4194304, 8388608,
    12582912, 16777216, 33554432, 67108864,
];
pub const ITERATIONS: [usize; 16] = [
    2048, 1024, 512, 512, 256, 128, 64, 32, 16, 8, 6, 6, 4, 4, 4, 2,
];

#[thread_local]
static MAP_LIST: [AtomicPtr<MapHeader>; 16] = [
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
];

#[thread_local]
static MAP_ALLOCATION_LIST: [AtomicUsize; 16] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

pub static TOTAL_USAGE: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

static IS_LAZY_INIT_ACTIVE: AtomicBool = AtomicBool::new(false);

static TRIM_THRESHOLD_PERCENT: LazyLock<usize> = LazyLock::new(|| {
    IS_LAZY_INIT_ACTIVE.store(true, Ordering::Relaxed);

    let result = env::var("OXIDALLOC_TRIM")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(75);

    IS_LAZY_INIT_ACTIVE.store(false, Ordering::Relaxed);
    result
});

#[repr(C, align(16))]
struct MapHeader {
    pub size: usize,
    pub magic: u64,
    pub next: *mut MapHeader,
}

fn bulk_allocate(count: usize, class: usize) -> bool {
    let user_size = SIZE_CLASSES[class];
    let header_size = std::mem::size_of::<MapHeader>();
    let block_size = header_size + user_size;
    let total_mmap_size = block_size * count;

    let chunk = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            total_mmap_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    MAP_ALLOCATION_LIST[class].fetch_add(count, Ordering::Relaxed);

    if chunk == libc::MAP_FAILED {
        return false;
    }

    if class >= 9 {
        unsafe { madvise(chunk, total_mmap_size, libc::MADV_HUGEPAGE) };
    }

    let mut prev: *mut MapHeader = std::ptr::null_mut();

    for i in (0..count).rev() {
        let header = (chunk as usize + i * block_size) as *mut MapHeader;

        unsafe {
            (*header).size = user_size;
            (*header).magic = MAGIC;
            (*header).next = prev;
        }

        prev = header;
    }

    MAP_LIST[class].store(prev, std::sync::atomic::Ordering::Relaxed);
    TOTAL_ALLOCATED.fetch_add(total_mmap_size, Ordering::Relaxed);
    TOTAL_USAGE.fetch_add(total_mmap_size, Ordering::Relaxed);

    true
}

pub fn match_size_class(size: usize) -> Option<usize> {
    match size {
        0..=512 => Some(0),
        513..=4096 => Some(1),
        4097..=6144 => Some(2),
        6145..=8192 => Some(3),
        8193..=16384 => Some(4),
        16385..=32768 => Some(5),
        32769..=65536 => Some(6),
        65537..=262144 => Some(7),
        262145..=1048576 => Some(8),
        1048577..=2097152 => Some(9),
        2097153..=4194304 => Some(10),
        4194305..=8388608 => Some(11),
        8388609..=12582912 => Some(12),
        12582913..=16777216 => Some(13),
        16777217..=33554432 => Some(14),
        33554433..=67108864 => Some(15),
        _ => None,
    }
}

fn pop_from_list(class: usize) -> *mut c_void {
    unsafe {
        loop {
            let header = MAP_LIST[class].load(Ordering::Acquire);
            if header.is_null() {
                return std::ptr::null_mut();
            }

            let next = (*header).next;

            if MAP_LIST[class]
                .compare_exchange(header, next, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                (*header).next = std::ptr::null_mut();
                return (header as *mut u8).add(std::mem::size_of::<MapHeader>()) as *mut c_void;
            }
        }
    }
}

pub fn big_alloc(size: usize) -> *mut c_void {
    unsafe {
        let total_size = size + std::mem::size_of::<MapHeader>();

        let chunk = libc::mmap(
            std::ptr::null_mut(),
            total_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );

        if chunk == libc::MAP_FAILED {
            return std::ptr::null_mut();
        }

        TOTAL_ALLOCATED.fetch_add(total_size, Ordering::Relaxed);

        madvise(chunk, total_size, MADV_HUGEPAGE);

        let header = chunk as *mut MapHeader;
        (*header).magic = MAGIC;
        (*header).size = size;
        (*header).next = std::ptr::null_mut();
        (header as *mut u8).add(std::mem::size_of::<MapHeader>()) as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: size_t) -> *mut c_void {
    let class = match match_size_class(size) {
        Some(class) => class,
        None => {
            return big_alloc(size);
        }
    };

    let ptr = pop_from_list(class);
    if !ptr.is_null() {
        TOTAL_USAGE.fetch_sub(size, Ordering::Relaxed);
        MAP_ALLOCATION_LIST[class].fetch_sub(1, Ordering::Relaxed);
        return ptr;
    }

    for _ in 0..3 {
        if bulk_allocate(ITERATIONS[class], class) {
            TOTAL_USAGE.fetch_sub(size, Ordering::Relaxed);
            MAP_ALLOCATION_LIST[class].fetch_sub(1, Ordering::Relaxed);
            return pop_from_list(class);
        }
    }

    std::ptr::null_mut()
}

fn trim() {
    if IS_LAZY_INIT_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    let usage = TOTAL_USAGE.load(Ordering::Relaxed);
    let allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);
    let trim_threshold = *TRIM_THRESHOLD_PERCENT;

    if allocated == 0 {
        return;
    }

    if (usage * 100) / allocated > trim_threshold {
        for allocation_class in (0..MAP_ALLOCATION_LIST.len()).rev() {
            let trim_target_count = ITERATIONS[allocation_class];

            if allocation_class == 0 {
                continue;
            }

            if MAP_ALLOCATION_LIST[allocation_class].load(Ordering::Relaxed) <= trim_target_count {
                continue;
            }

            while MAP_ALLOCATION_LIST[allocation_class].load(Ordering::Relaxed) > trim_target_count
            {
                let header = MAP_LIST[allocation_class].load(Ordering::Acquire);

                if header.is_null() {
                    break;
                }

                let next = unsafe { (*header).next };

                unsafe {
                    loop {
                        if MAP_LIST[allocation_class]
                            .compare_exchange(header, next, Ordering::Release, Ordering::Acquire)
                            .is_ok()
                        {
                            let size = (*header).size;
                            let total = std::mem::size_of::<MapHeader>() + size;
                            (*header).next = std::ptr::null_mut();
                            MAP_ALLOCATION_LIST[allocation_class].fetch_sub(1, Ordering::Relaxed);

                            let released_successfully =
                                libc::munmap(header as *mut c_void, total) == 0;

                            if released_successfully {
                                TOTAL_ALLOCATED.fetch_sub(total, Ordering::Relaxed);
                                TOTAL_USAGE.fetch_sub(total, Ordering::Relaxed);
                            } else {
                                libc::madvise(header as *mut c_void, total, libc::MADV_DONTNEED);
                            }

                            break;
                        }
                    }
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let header = unsafe { (ptr as *mut MapHeader).sub(1) };
    if unsafe { (*header).magic } != MAGIC {
        return;
    }

    let size = unsafe { (*header).size };
    let total = std::mem::size_of::<MapHeader>() + size;

    let class = match match_size_class(size) {
        Some(class) => class,
        None => unsafe {
            if munmap(header as *mut c_void, total) != 0 {
                (*header).magic = 0;
                madvise(header as *mut c_void, total, MADV_DONTNEED);
            }
            return;
        },
    };

    unsafe {
        loop {
            let head = MAP_LIST[class].load(Ordering::Acquire);
            (*header).next = head;
            if MAP_LIST[class]
                .compare_exchange(head, header, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
    }

    TOTAL_USAGE.fetch_add(total, Ordering::Relaxed);
    MAP_ALLOCATION_LIST[class].fetch_add(1, Ordering::Relaxed);

    trim();
}

#[unsafe(no_mangle)]
pub extern "C" fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void {
    if ptr.is_null() {
        return malloc(new_size);
    }

    if new_size == 0 {
        free(ptr);
        return std::ptr::null_mut();
    }

    unsafe {
        let header = (ptr as *mut u8).sub(std::mem::size_of::<MapHeader>()) as *mut MapHeader;
        if (*header).magic != MAGIC {
            return std::ptr::null_mut();
        }

        let old_size = (*header).size;
        let new_ptr = malloc(new_size);
        if new_ptr.is_null() {
            return std::ptr::null_mut();
        }

        let copy_size = old_size.min(new_size);
        std::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, copy_size);

        free(ptr);
        new_ptr
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(nmemb: size_t, size: size_t) -> *mut c_void {
    let total = match nmemb.checked_mul(size) {
        Some(t) => t,
        None => return std::ptr::null_mut(),
    };

    let ptr = malloc(total);
    if !ptr.is_null() {
        unsafe {
            std::ptr::write_bytes(ptr as *mut u8, 0, total);
        }
    }
    ptr
}

#[test]
fn bench_allocator() {
    use std::{hint::black_box, time::Instant};

    let iterations = 1_000_000;

    // Warm up
    for _ in black_box(0..1000) {
        let ptr = black_box(malloc(100));
        black_box(free(ptr));
    }

    // Bench small allocations
    let start = Instant::now();
    for _ in black_box(0..iterations) {
        let ptr = black_box(malloc(100));
        black_box(free(ptr));
    }
    let small_time = start.elapsed();
    println!(
        "Small (100B): {:?} ({:.2} ns/op)",
        small_time,
        small_time.as_nanos() as f64 / iterations as f64
    );

    // Bench medium allocations
    let start = Instant::now();
    for _ in black_box(0..iterations) {
        let ptr = black_box(malloc(8192));
        black_box(free(ptr));
    }
    let med_time = start.elapsed();
    println!(
        "Medium (8KB): {:?} ({:.2} ns/op)",
        med_time,
        med_time.as_nanos() as f64 / iterations as f64
    );

    // Bench large allocations
    let start = Instant::now();
    for _ in black_box(0..10000) {
        let ptr = black_box(malloc(1024 * 1024 * 2));
        black_box(free(ptr));
    }
    let large_time = start.elapsed();
    println!(
        "Large (1MB): {:?} ({:.2} ns/op)",
        large_time,
        large_time.as_nanos() as f64 / 10000.0
    );
}
