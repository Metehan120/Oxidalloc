#![allow(unsafe_op_in_unsafe_fn)]

use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    HEADER_SIZE, MAGIC, OxHeader,
    va::{align_to, bitmap::VA_MAP, va_helper::is_ours},
};
use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

pub struct BigAllocation {
    inner: *mut c_void,
    size: usize,
    stamp: usize,
}
unsafe impl Send for BigAllocation {}

pub const COUNT: usize = 10;

pub static BIG_LIST: Mutex<[BigAllocation; COUNT]> = Mutex::new(
    [const {
        BigAllocation {
            inner: null_mut(),
            size: 0,
            stamp: 0,
        }
    }; COUNT],
);

pub static OX_BIG_STAMP: OnceLock<Instant> = OnceLock::new();
pub static OX_BIG_CR_STAMP: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_BIG_CALLS: AtomicUsize = AtomicUsize::new(0);

pub fn get_big_clock() -> &'static Instant {
    OX_BIG_STAMP.get_or_init(|| Instant::now())
}

pub unsafe fn find_biggest() -> Option<usize> {
    let big_list = BIG_LIST.lock().ok()?;

    let mut index = None;
    let mut biggest = 0;

    // Loop through the big_list to find the biggest allocation, ~10 iterations: basically free
    for i in 0..COUNT {
        if !big_list[i].inner.is_null() && big_list[i].size > biggest {
            biggest = big_list[i].size;
            index = Some(i);
        }
    }

    index
}

pub unsafe fn add_to_list(size: usize, ptr: *mut c_void) -> bool {
    let mut big_list = match BIG_LIST.lock() {
        Ok(list) => list,
        Err(_) => return false,
    };

    let stamp = OX_BIG_CR_STAMP.load(Ordering::Relaxed);

    for slot in big_list.iter_mut() {
        if slot.size == 0 {
            slot.inner = ptr;
            slot.size = size;
            slot.stamp = stamp;
            return true;
        }
    }

    false
}

pub unsafe fn check_list(needed: usize) -> *mut c_void {
    let mut big_list = match BIG_LIST.lock() {
        Ok(list) => list,
        Err(_) => return null_mut(),
    };

    let cr_stamp = OX_BIG_CR_STAMP.load(Ordering::Relaxed);

    for slot in big_list.iter_mut() {
        if slot.inner.is_null() {
            continue;
        }

        if cr_stamp.saturating_sub(slot.stamp) > 1024 {
            let ptr = slot.inner;
            slot.inner = null_mut();
            slot.size = 0;
            slot.stamp = 0;
            big_free_inner(ptr);
            continue;
        }

        if slot.size >= needed {
            let ptr = slot.inner;
            slot.inner = null_mut();
            slot.size = 0;
            slot.stamp = 0;
            return ptr;
        }
    }

    null_mut()
}

pub unsafe fn purge_block(index: usize) {
    let mut big_list = match BIG_LIST.lock() {
        Ok(list) => list,
        Err(_) => return,
    };

    let ptr = big_list[index].inner;

    big_list[index].inner = null_mut();
    big_list[index].size = 0;
    big_list[index].stamp = 0;

    if !ptr.is_null() {
        let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;
        if is_ours(header as usize) {
            big_free_inner(ptr);
        }
    }
}

pub unsafe fn purge_list() -> bool {
    let mut big_list = match BIG_LIST.lock() {
        Ok(list) => list,
        Err(_) => return false,
    };

    for slot in big_list.iter_mut() {
        let ptr = slot.inner;

        slot.inner = null_mut();
        slot.size = 0;
        slot.stamp = 0;

        if !ptr.is_null() {
            let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;
            if is_ours(header as usize) {
                big_free_inner(ptr);
            }
        }
    }

    true
}

pub unsafe fn big_malloc(size: usize) -> *mut u8 {
    let cached = check_list(size);
    if !cached.is_null() {
        return cached as *mut u8;
    }

    big_malloc_inner(size)
}

pub unsafe fn big_free(ptr: *mut c_void) {
    let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut OxHeader;
    let payload_size = (*header).size as usize;

    if TOTAL_BIG_CALLS.load(Ordering::Relaxed) % 2000 == 0 {
        purge_list();
    }

    if payload_size <= 1024 * 1024 * 13 {
        if add_to_list(payload_size, ptr) {
            return;
        }

        match find_biggest() {
            None => {
                purge_list();
                big_free_inner(ptr);
            }
            Some(index) => purge_block(index),
        }

        return;
    }

    big_free_inner(ptr);
}

pub unsafe fn big_malloc_inner(size: usize) -> *mut u8 {
    TOTAL_BIG_CALLS.fetch_add(1, Ordering::Relaxed);
    // Align size to the page size so we don't explode later
    let aligned_total = align_to(size + HEADER_SIZE, 4096);

    // Reserve virtual space first
    let hint = match VA_MAP.alloc(aligned_total) {
        Some(hint) => hint,
        None => return null_mut(),
    };

    let actual_ptr = match mmap_anonymous(
        hint as *mut c_void,
        aligned_total,
        ProtFlags::WRITE | ProtFlags::READ,
        MapFlags::PRIVATE | MapFlags::FIXED,
    ) {
        Ok(ptr) => ptr,
        Err(_) => {
            VA_MAP.free(hint, aligned_total);
            return null_mut();
        }
    } as *mut OxHeader;

    let _ = madvise(
        actual_ptr as *mut c_void,
        aligned_total,
        Advice::LinuxHugepage,
    );

    let current = OX_BIG_CR_STAMP.load(Ordering::Relaxed);

    // Initialize the header
    (*actual_ptr).size = size as u64;
    (*actual_ptr).magic = MAGIC;
    (*actual_ptr).in_use = 1;
    (*actual_ptr).life_time = current;

    (actual_ptr as *mut u8).add(HEADER_SIZE)
}

pub unsafe fn big_free_inner(ptr: *mut c_void) {
    TOTAL_BIG_CALLS.fetch_add(1, Ordering::Relaxed);
    let header = (ptr as *mut OxHeader).sub(1);
    let payload_size = (*header).size as usize;

    // Align size back to original size
    let total_size = align_to(payload_size + HEADER_SIZE, 4096);

    // Make the header look free before we potentially lose write access.
    (*header).in_use = 0;
    (*header).magic = 0;

    // If this fails (e.g. under a restrictive sandbox), fall back to `madvise(DONTNEED)`.
    let remap_result = mmap_anonymous(
        header as *mut c_void,
        total_size,
        ProtFlags::empty(),
        MapFlags::PRIVATE | MapFlags::FIXED | MapFlags::NORESERVE,
    );

    if remap_result.is_err() {
        match madvise(header as *mut c_void, total_size, Advice::LinuxDontNeed) {
            Ok(_) => (),
            Err(errno) => match errno.raw_os_error() {
                0 => (),
                _ => return,
            },
        };
    }

    VA_MAP.free(header as usize, total_size);
}
