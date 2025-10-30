#![feature(thread_local)]

use libc::{madvise, size_t};
use std::ffi::c_void;

const MAGIC: u64 = 0x01B01698BF;

pub const SIZE_CLASSES: [usize; 11] = [
    4096, 6144, 8192, 16384, 32768, 65536, 262144, 1048576, 12582912, 33554432, 67108864,
];
static ITERATIONS: [usize; 11] = [1024, 512, 512, 256, 128, 64, 32, 16, 8, 4, 2];

#[thread_local]
static mut FREE_LISTS: [*mut Map; 11] = [std::ptr::null_mut(); 11];

#[thread_local]
pub static mut TOTAL_USAGE: u64 = 0;
#[thread_local]
pub static mut USAGE_LIST: [u64; 11] = [0; 11];
pub static mut MAX_USAGE: [u64; 11] = [
    1024 * 64,
    512 * 64,
    512 * 64,
    256 * 64,
    128 * 32,
    64 * 32,
    32 * 32,
    16 * 16,
    8 * 16,
    4 * 8,
    2 * 8,
];

#[derive(Debug)]
struct Map {
    next: *mut Map,
}

#[repr(align(16))]
struct Header {
    magic: u64,
    size: usize,
}

pub fn match_size_class(size: usize) -> Option<usize> {
    match size {
        0..=4096 => Some(0),
        4097..=6144 => Some(1),
        6145..=8192 => Some(2),
        8193..=16384 => Some(3),
        16385..=32768 => Some(4),
        32769..=65536 => Some(5),
        65537..=262144 => Some(6),
        262145..=1048576 => Some(7),
        1048577..=12582912 => Some(8),
        12582913..=33554432 => Some(9),
        33554433..=67108864 => Some(10),
        _ => None,
    }
}

fn bulk_allocate(size_class: usize, num_blocks: usize) {
    let block_size = SIZE_CLASSES[size_class] + size_of::<Header>();
    let total_mmap_size = block_size * num_blocks;

    unsafe { TOTAL_USAGE += total_mmap_size as u64 };

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
        return;
    }

    for i in 0..num_blocks {
        let block = (chunk as usize + i * block_size) as *mut Map;
        let next = if i < num_blocks - 1 {
            (chunk as usize + (i + 1) * block_size) as *mut Map
        } else {
            std::ptr::null_mut()
        };

        unsafe {
            (*block).next = next;
        }
    }

    unsafe {
        FREE_LISTS[size_class] = chunk as *mut Map;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: size_t) -> *mut c_void {
    match match_size_class(size) {
        Some(id) => unsafe {
            if FREE_LISTS[id].is_null() {
                bulk_allocate(id, ITERATIONS[id]);
                if FREE_LISTS[id].is_null() {
                    return std::ptr::null_mut();
                }
            }

            let block = FREE_LISTS[id];
            FREE_LISTS[id] = (*block).next;
            USAGE_LIST[id] += SIZE_CLASSES[id] as u64;

            let header = block as *mut Header;
            (*header).magic = MAGIC;
            (*header).size = SIZE_CLASSES[id];

            (header as *mut u8).add(size_of::<Header>()) as *mut c_void
        },
        None => {
            if size == 0 {
                return std::ptr::null_mut();
            }

            let total_size = size_of::<Header>() + size;
            let addr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    total_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };

            if addr == libc::MAP_FAILED {
                return std::ptr::null_mut();
            }

            unsafe {
                let header = addr as *mut Header;
                (*header).magic = MAGIC;
                (*header).size = size;
                (header as *mut u8).add(size_of::<Header>()) as *mut c_void
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }

    unsafe {
        let header = (ptr as *mut Header).sub(1);

        if (*header).magic != MAGIC {
            return;
        }

        let size = (*header).size;

        if let Some(id) = match_size_class(size) {
            if USAGE_LIST[id] >= MAX_USAGE[id] {
                let total_size = size + size_of::<Header>();
                TOTAL_USAGE -= total_size as u64;
                USAGE_LIST[id] -= SIZE_CLASSES[id] as u64;
                libc::munmap(header as *mut c_void, total_size);
                return;
            }

            let block = header as *mut Map;

            if SIZE_CLASSES[id] >= 1048576 {
                madvise(ptr, size, libc::MADV_DONTNEED);
            } else if SIZE_CLASSES[id] >= 65536 {
                madvise(ptr, size, libc::MADV_FREE);
            }

            (*block).next = FREE_LISTS[id];
            FREE_LISTS[id] = block;
        } else {
            let total_size = size + size_of::<Header>();
            TOTAL_USAGE -= total_size as u64;
            libc::munmap(header as *mut c_void, total_size);
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
        let header = (ptr as *mut Header).sub(1);
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
    let total = nmemb.saturating_mul(size);
    let ptr = malloc(total);
    if !ptr.is_null() {
        unsafe {
            std::ptr::write_bytes(ptr as *mut u8, 0, total);
        }
    }
    ptr
}

#[test]
fn test() {
    let ptr = malloc(10);
    println!("Allocated");
    assert!(!ptr.is_null());
    free(ptr);
}
