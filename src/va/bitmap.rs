#![allow(unsafe_op_in_unsafe_fn)]

use crate::{OX_MAX_RESERVATION, OxidallocError};
use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous};
use std::{
    cell::UnsafeCell,
    os::raw::c_void,
    ptr::{null_mut, write},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering},
};

pub static LATEST_TRIED: AtomicUsize = AtomicUsize::new(0);
pub static RESERVE: AtomicU8 = AtomicU8::new(0);

pub unsafe fn get_va_from_kernel() -> (*mut c_void, usize, usize) {
    const MIN_RESERVE: usize = 1024 * 1024 * 256;
    #[allow(non_snake_case)]
    let MAX_SIZE =
        if LATEST_TRIED.load(Ordering::Relaxed) != 0 && RESERVE.load(Ordering::Relaxed) > 5 {
            LATEST_TRIED.load(Ordering::Relaxed)
        } else {
            RESERVE.fetch_add(1, Ordering::Relaxed);
            OX_MAX_RESERVATION.load(Ordering::Relaxed)
        };

    let mut size = MAX_SIZE;

    if MAX_SIZE < MIN_RESERVE {
        size = MIN_RESERVE;
    }

    loop {
        let probe = mmap_anonymous(
            null_mut(),
            size,
            ProtFlags::empty(),
            MapFlags::PRIVATE | MapFlags::NORESERVE,
        );

        match probe {
            Ok(output) => {
                if size > LATEST_TRIED.load(Ordering::Relaxed) {
                    LATEST_TRIED.store(size, Ordering::Relaxed);
                }
                return (output, (output as usize) + size, size);
            }
            Err(err) => {
                if size <= MIN_RESERVE {
                    OxidallocError::VAIinitFailed.log_and_abort(
                        0 as *mut c_void,
                        "Init failed during Segment Allocation: No available VA reserve",
                        Some(err),
                    )
                }

                size /= 2;
            }
        }
    }
}

pub const BLOCK_SIZE: usize = 4096;

pub static VA_MAP: VaBitmap = VaBitmap::new();

thread_local! {
    static LATEST_SEGMENT: UnsafeCell<*mut Segment> = UnsafeCell::new(null_mut());
}

pub struct Segment {
    next: *mut Segment,
    va_start: usize,
    va_end: usize,
    pub map: AtomicPtr<AtomicU64>,
    hint: AtomicUsize,
    pub map_len: usize,
}

// Half of written by AI, I was not able to complete it at the time so there should be many ways to improve it.
// TODO: Catch errors and optimize it
// - Metehan
pub struct VaBitmap {
    map: AtomicPtr<Segment>,
    lock: AtomicBool,
}

impl VaBitmap {
    pub const fn new() -> Self {
        Self {
            map: AtomicPtr::new(null_mut()),
            lock: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    pub unsafe fn is_ours(&self, addr: usize) -> bool {
        let latest = LATEST_SEGMENT.with(|latest| *latest.get());

        if !latest.is_null() {
            let s = &*latest;
            if addr >= s.va_start && addr < s.va_end {
                return true;
            }
        }

        let mut curr = self.map.load(Ordering::Acquire);
        while !curr.is_null() {
            let s = unsafe { &*curr };
            if addr >= s.va_start && addr < s.va_end {
                LATEST_SEGMENT.with(|ptr| ptr.get().write(curr));
                return true;
            }
            curr = s.next;
        }

        false
    }

    pub unsafe fn grow(&self) -> Option<*mut Segment> {
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }

        let (user_va, end, total_size) = get_va_from_kernel();
        let bit_count = total_size / BLOCK_SIZE;
        let map_len = (bit_count + 63) / 64;
        let map_bytes = map_len * size_of::<u64>();

        let map_raw = match mmap_anonymous(
            null_mut(),
            map_bytes,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::PRIVATE,
        ) {
            Ok(ptr) => ptr,
            Err(_) => {
                self.lock.store(false, Ordering::Release);
                return None;
            }
        };

        let seg_ptr = match mmap_anonymous(
            null_mut(),
            size_of::<Segment>(),
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::PRIVATE,
        ) {
            Ok(ptr) => ptr as *mut Segment,
            Err(_) => {
                self.lock.store(false, Ordering::Release);
                return None;
            }
        };

        let old_head = self.map.load(Ordering::Relaxed);
        write(
            seg_ptr,
            Segment {
                next: old_head,
                va_start: user_va as usize,
                va_end: end,
                map: AtomicPtr::new(map_raw as *mut AtomicU64),
                hint: AtomicUsize::new(0),
                map_len,
            },
        );

        self.map.store(seg_ptr, Ordering::Release);

        self.lock.store(false, Ordering::Release);
        Some(seg_ptr)
    }

    pub unsafe fn alloc(&self, size: usize) -> Option<usize> {
        if size == 0 {
            return None;
        }
        let needed = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;

        let mut curr = self.map.load(Ordering::Acquire);
        if curr.is_null() {
            match self.grow() {
                Some(new) => curr = new,
                None => return None,
            };
        }

        while !curr.is_null() {
            let segment = unsafe { &*curr };
            let res = if needed == 1 {
                segment.alloc_single()
            } else {
                segment.alloc_multi(needed)
            };

            if res.is_some() {
                return res;
            }
            curr = unsafe { (*curr).next };
        }

        let new_seg_ptr = self.grow()?;
        let new_seg = unsafe { &*new_seg_ptr };
        if needed == 1 {
            new_seg.alloc_single()
        } else {
            new_seg.alloc_multi(needed)
        }
    }

    pub unsafe fn free(&self, addr: usize, size: usize) {
        if addr == 0 || size == 0 {
            return;
        }

        let mut curr = self.map.load(Ordering::Acquire);
        while !curr.is_null() {
            let s = unsafe { &*curr };
            if addr >= s.va_start && addr < s.va_end {
                s.free(addr, size);
                return;
            }
            curr = s.next;
        }
    }

    pub unsafe fn realloc_inplace(
        &self,
        addr: usize,
        old_size: usize,
        new_size: usize,
    ) -> Option<usize> {
        let mut curr = self.map.load(Ordering::Acquire);
        while !curr.is_null() {
            let s = unsafe { &*curr };
            if addr >= s.va_start && addr < s.va_end {
                let va_size = s.realloc_inplace(addr, old_size, new_size);
                return va_size;
            }
            curr = s.next;
        }
        None
    }
}

impl Segment {
    #[inline(always)]
    unsafe fn get_map(&self) -> &[AtomicU64] {
        std::slice::from_raw_parts(self.map.load(Ordering::Acquire), self.map_len)
    }

    #[inline(always)]
    fn max_bits(&self) -> usize {
        (self.va_end - self.va_start) / BLOCK_SIZE
    }

    fn alloc_single(&self) -> Option<usize> {
        let map = unsafe { self.get_map() };
        let total_bits = self.max_bits();
        if total_bits == 0 {
            return None;
        }

        let chunks = (total_bits + 63) / 64;
        let start_chunk = self.hint.load(Ordering::Relaxed) % chunks;
        let last_valid_bits = total_bits % 64;

        for (range_start, range_end) in [(start_chunk, chunks), (0, start_chunk)] {
            for i in range_start..range_end {
                let mut chunk = map[i].load(Ordering::Relaxed);
                if i == chunks - 1 && last_valid_bits != 0 {
                    chunk |= !((1u64 << last_valid_bits) - 1);
                }

                if chunk == u64::MAX {
                    continue;
                }

                let bit = (!chunk).trailing_zeros();
                let mask = 1u64 << bit;

                if (map[i].fetch_or(mask, Ordering::Acquire) & mask) == 0 {
                    self.hint.store(i, Ordering::Relaxed);
                    let global_idx = (i * 64) + bit as usize;
                    if global_idx >= total_bits {
                        map[i].fetch_and(!mask, Ordering::Release);
                        continue;
                    }

                    let addr = self.va_start + (global_idx * BLOCK_SIZE);
                    return Some(addr);
                }
            }
        }
        None
    }

    fn alloc_multi(&self, count: usize) -> Option<usize> {
        let map = unsafe { self.get_map() };
        let total_bits = self.max_bits();
        if total_bits == 0 || count > total_bits {
            return None;
        }

        let start_bit = (self.hint.load(Ordering::Relaxed) * 64) % total_bits;

        for (range_start, range_end) in [(start_bit, total_bits), (0, start_bit)] {
            let mut current_run = 0;
            let mut run_start = 0;

            for global_bit in range_start..range_end {
                let chunk = map[global_bit / 64].load(Ordering::Relaxed);
                if (chunk & (1u64 << (global_bit % 64))) != 0 {
                    current_run = 0;
                } else {
                    if current_run == 0 {
                        run_start = global_bit;
                    }
                    current_run += 1;

                    if current_run == count {
                        if self.try_claim(run_start, count) {
                            self.hint.store(run_start / 64, Ordering::Relaxed);
                            return Some(self.va_start + (run_start * BLOCK_SIZE));
                        }
                        current_run = 0;
                    }
                }
            }
        }
        None
    }

    fn try_claim(&self, start_idx: usize, count: usize) -> bool {
        let map = unsafe { self.get_map() };
        let total_bits = self.max_bits();

        if start_idx + count > total_bits {
            return false;
        }

        for k in 0..count {
            let idx = start_idx + k;
            let chunk_idx = idx / 64;
            let mask = 1u64 << (idx % 64);

            let prev = map[chunk_idx].fetch_or(mask, Ordering::Acquire);
            if (prev & mask) != 0 {
                self.rollback(start_idx, k);
                return false;
            }
        }
        true
    }

    fn rollback(&self, start_idx: usize, count: usize) {
        let map = unsafe { self.get_map() };
        for k in 0..count {
            let idx = start_idx + k;
            let mask = 1u64 << (idx % 64);
            map[idx / 64].fetch_and(!mask, Ordering::Release);
        }
    }

    pub fn free(&self, addr: usize, size: usize) {
        let start = self.va_start;
        let end = self.va_end;
        if addr < start || addr >= end {
            return;
        }

        let total_bits = self.max_bits();
        if total_bits == 0 {
            return;
        }

        let offset = addr - start;
        let start_idx = offset / BLOCK_SIZE;
        if start_idx >= total_bits {
            return;
        }

        let mut count = (size / BLOCK_SIZE) + usize::from(size % BLOCK_SIZE != 0);
        if start_idx + count > total_bits {
            count = total_bits - start_idx;
        }

        self.rollback(start_idx, count);

        let chunk = start_idx / 64;
        if chunk < self.hint.load(Ordering::Relaxed) {
            self.hint.store(chunk, Ordering::Relaxed);
        }
    }

    pub fn realloc_inplace(&self, addr: usize, old_size: usize, new_size: usize) -> Option<usize> {
        let old_blocks = (old_size + BLOCK_SIZE - 1) / BLOCK_SIZE;
        let new_blocks = (new_size + BLOCK_SIZE - 1) / BLOCK_SIZE;

        if new_blocks == old_blocks {
            return Some(old_blocks * BLOCK_SIZE);
        }

        if new_blocks < old_blocks {
            let shrink_start_bit = (addr - self.va_start) / BLOCK_SIZE + new_blocks;
            let count_to_free = old_blocks - new_blocks;
            self.rollback(shrink_start_bit, count_to_free);
            return Some(new_blocks * BLOCK_SIZE);
        }

        let start_va = self.va_start;
        let offset = addr - start_va;
        let start_bit = offset / BLOCK_SIZE;
        let growth_start_bit = start_bit + old_blocks;
        let additional_needed = new_blocks - old_blocks;

        if growth_start_bit + additional_needed > self.max_bits() {
            return None;
        }

        if self.try_claim(growth_start_bit, additional_needed) {
            Some(new_blocks * BLOCK_SIZE)
        } else {
            None
        }
    }
}
