#![feature(thread_local)]

use libc::{MADV_DONTNEED, madvise, munmap, size_t};
use std::{
    os::raw::c_void,
    sync::atomic::{AtomicPtr, Ordering},
};

const MAGIC: u64 = 0x01B01698BF;

pub const SIZE_CLASSES: [usize; 12] = [
    512, 4096, 6144, 8192, 16384, 32768, 65536, 262144, 1048576, 12582912, 33554432, 67108864,
];
pub const ITERATIONS: [usize; 12] = [8192, 4096, 2048, 1024, 512, 128, 64, 32, 16, 8, 4, 2];

#[thread_local]
static mut MAP_LIST: [AtomicPtr<Header>; 12] = [
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

// Will be added in future
pub const TRIM_LEVEL: usize = 100;

#[repr(C, align(16))]
struct Header {
    pub size: usize,
    pub magic: u64,
    pub next: *mut Header,
}

fn bulk_allocate(count: usize, class: usize) -> bool {
    let user_size = SIZE_CLASSES[class];
    let header_size = std::mem::size_of::<Header>();
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

    if chunk == libc::MAP_FAILED {
        return false;
    }

    let mut prev: *mut Header = std::ptr::null_mut();

    for i in (0..count).rev() {
        let header = (chunk as usize + i * block_size) as *mut Header;

        unsafe {
            (*header).size = user_size;
            (*header).magic = MAGIC;
            (*header).next = prev;
        }

        prev = header;
    }

    unsafe {
        MAP_LIST[class].store(prev, std::sync::atomic::Ordering::Relaxed);
    };

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
        1048577..=12582912 => Some(9),
        12582913..=33554432 => Some(10),
        33554433..=67108864 => Some(11),
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
                (*header).magic = MAGIC;
                return (header as *mut u8).add(std::mem::size_of::<Header>()) as *mut c_void;
            }
        }
    }
}

pub fn big_alloc(size: usize) -> *mut c_void {
    unsafe {
        let total_size = size + std::mem::size_of::<Header>();

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

        let header = chunk as *mut Header;
        (*header).magic = MAGIC;
        (*header).size = size;
        (*header).next = std::ptr::null_mut();
        (header as *mut u8).add(std::mem::size_of::<Header>()) as *mut c_void
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
        return ptr;
    }

    for _ in 0..3 {
        if bulk_allocate(ITERATIONS[class], class) {
            return pop_from_list(class);
        }
    }

    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    let header = unsafe { (ptr as *mut Header).sub(1) };

    let magic = match std::panic::catch_unwind(|| unsafe { (*header).magic }) {
        Ok(m) => m,
        Err(_) => return,
    };

    if magic != MAGIC {
        return;
    }

    let size = unsafe { (*header).size };
    let total = std::mem::size_of::<Header>() + size;

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
        match TRIM_LEVEL {
            1 => {
                let _ = munmap(header as *mut c_void, total);
                (*header).magic = 0;
            }
            2..=99 => loop {
                if class >= 6 {
                    madvise(header as *mut c_void, total, libc::MADV_FREE);
                }
                let head = MAP_LIST[class].load(Ordering::Acquire);
                (*header).next = head;
                if MAP_LIST[class]
                    .compare_exchange(head, header, Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            },
            _ => loop {
                let head = MAP_LIST[class].load(Ordering::Acquire);
                (*header).next = head;
                if MAP_LIST[class]
                    .compare_exchange(head, header, Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            },
        }
    }
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
        let header = (ptr as *mut u8).sub(std::mem::size_of::<Header>()) as *mut Header;
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
    let total = nmemb * size;
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
        let ptr = black_box(malloc(1048576));
        black_box(free(ptr));
    }
    let large_time = start.elapsed();
    println!(
        "Large (1MB): {:?} ({:.2} ns/op)",
        large_time,
        large_time.as_nanos() as f64 / 10000.0
    );
}
