use crate::{
    OX_MAX_RESERVATION, OxidallocError,
    internals::{lock::SerialLock, once::Once},
    sys::memory_system::{MMapFlags, MProtFlags, MemoryFlags, mmap_memory},
    va::{
        align_to,
        bootstrap::{boot_strap, init_alloc_random},
    },
};
use libc::getrandom;
use std::{
    hint::{likely, unlikely},
    mem::size_of,
    os::raw::c_void,
    ptr::{null_mut, write},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering},
};

const MAX_RANDOM_MB: usize = 64;
const MAX_RANDOM_BYTES: usize = MAX_RANDOM_MB * 1024 * 1024;
const MAX_RANDOM_BLOCKS: usize = MAX_RANDOM_BYTES / BLOCK_SIZE;

pub static LATEST_TRIED: AtomicUsize = AtomicUsize::new(0);
pub static RESERVE: AtomicU8 = AtomicU8::new(0);
pub static ONCE: Once = Once::new();
pub static ONCE_PROTECTION: Once = Once::new();
pub static mut BASE_HINT: usize = 0;
pub static mut BASE_INIT: bool = false;
pub static mut LOOP: u8 = 0;

#[thread_local]
pub static mut ALLOC_RNG: u64 = 0;
#[thread_local]
pub static THREAD_ONCE_PROTECTION: Once = Once::new();

#[inline(always)]
unsafe fn alloc_random() -> usize {
    THREAD_ONCE_PROTECTION.call_once(|| {
        init_alloc_random();
    });

    ALLOC_RNG = ALLOC_RNG.wrapping_add(1);
    let x = ALLOC_RNG;
    let mut z = x.wrapping_add(0x9E3779B97F4A7C15);
    z ^= z >> 30;
    z = z.wrapping_mul(0xbf58476d1ce4e5b9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94d049bb133111eb);
    z ^= z >> 31;
    z as usize
}

// Security: Restored getrandom for strong ASLR on segment bases.
unsafe fn randomize_base_hint() {
    let max = LATEST_TRIED.load(Ordering::Relaxed);
    if unlikely(max == 0) {
        return;
    }

    let mut rand: usize = 0;
    // We use the kernel's RNG here to ensure the base address is not predictable
    let ret = getrandom(
        &mut rand as *mut usize as *mut c_void,
        size_of::<usize>(),
        0,
    );

    if unlikely(ret as usize != size_of::<usize>()) {
        OxidallocError::SecurityViolation.log_and_abort(
            null_mut(),
            "Failed to initialize random number generator",
            None,
        );
    }

    BASE_HINT = BASE_HINT.wrapping_add(align_to(rand % max, 4096));
}

pub unsafe fn get_va_from_kernel() -> (*mut c_void, usize, usize) {
    boot_strap();
    randomize_base_hint();

    const MIN_RESERVE: usize = CHUNK_SIZE;
    #[allow(non_snake_case)]
    let MAX_SIZE: usize = if likely(RESERVE.load(Ordering::Relaxed) > 3) {
        LATEST_TRIED.load(Ordering::Relaxed)
    } else {
        RESERVE.fetch_add(1, Ordering::Relaxed);
        OX_MAX_RESERVATION.load(Ordering::Relaxed)
    };

    let mut size = MAX_SIZE;

    if unlikely(MAX_SIZE < MIN_RESERVE) {
        size = MIN_RESERVE;
    }

    loop {
        let base_hint = BASE_HINT;

        let flags = if BASE_INIT {
            MemoryFlags::PRIVATE | MemoryFlags::NORESERVE | MemoryFlags::FIXED_NOREPLACE
        } else {
            MemoryFlags::PRIVATE | MemoryFlags::NORESERVE
        };

        let target = if BASE_INIT {
            base_hint as *mut c_void
        } else {
            null_mut()
        };

        let probe = mmap_memory(
            target,
            size,
            MMapFlags {
                prot: MProtFlags::NONE,
                map: flags,
            },
        );

        match probe {
            Ok(output) => {
                if size > LATEST_TRIED.load(Ordering::Relaxed) {
                    LATEST_TRIED.store(size, Ordering::Relaxed);
                }

                if unlikely(!BASE_INIT) {
                    BASE_HINT = output as usize;
                    BASE_INIT = true;
                } else {
                    BASE_HINT += size;
                }

                return (output, (output as usize) + size, size);
            }
            Err(err) => {
                if (size <= MIN_RESERVE) && !BASE_INIT {
                    OxidallocError::VAIinitFailed.log_and_abort(
                        null_mut(),
                        "Init failed during Segment Allocation: No available VA reserve",
                        Some(err),
                    )
                } else if size <= MIN_RESERVE {
                    BASE_INIT = false;
                    return (null_mut(), 0, 0);
                }

                BASE_HINT += size;
                size /= 2;
            }
        }
    }
}

pub const BLOCK_SIZE: usize = 4096;
pub static mut VA_MAP: VaBitmap = VaBitmap::new();

pub(crate) fn reset_fork_locks() {
    unsafe {
        VA_MAP.lock.reset_on_fork();
    }
}

pub(crate) fn reset_fork_onces() {
    ONCE.reset_at_fork();
    ONCE_PROTECTION.reset_at_fork();
}

pub const CHUNK_SIZE: usize = 1024 * 1024 * 1024 * 4;

// Zen 4>: 58/57bit | Zen <3: 48/47bit
const ENTRIES: usize = (1 << 48) / CHUNK_SIZE;

pub struct SegmentIndex {
    nodes: *mut usize,
}

impl SegmentIndex {
    pub unsafe fn new() -> Self {
        let size = ENTRIES * size_of::<usize>();

        let ptr = match mmap_memory(
            null_mut(),
            size,
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        ) {
            Ok(ptr) => ptr,
            Err(err) => OxidallocError::VAIinitFailed.log_and_abort(
                null_mut(),
                "Cannot allocate memory for RadixTree",
                Some(err),
            ),
        };

        Self {
            nodes: ptr as *mut usize,
        }
    }

    #[inline(always)]
    pub fn set_range(&self, start: usize, size: usize, seg_ptr: *mut Segment) {
        let start_idx = start / CHUNK_SIZE;
        let end_idx = start.saturating_add(size.saturating_sub(1)) / CHUNK_SIZE;
        let count = end_idx.saturating_sub(start_idx) + 1;
        let base = self.nodes;

        unsafe {
            for i in 0..count {
                *base.add(start_idx + i) = seg_ptr as usize;
            }
        }
    }

    #[inline(always)]
    pub fn get_segment(&self, addr: usize) -> *mut Segment {
        let idx = addr / CHUNK_SIZE;
        let base = self.nodes;
        if unlikely(base.is_null() || idx >= ENTRIES) {
            return null_mut();
        }
        unsafe { *base.add(idx) as *mut Segment }
    }

    pub unsafe fn check_collision(&self, start: usize, size: usize) -> bool {
        let start_idx = start / CHUNK_SIZE;
        let end_idx = start.saturating_add(size.saturating_sub(1)) / CHUNK_SIZE;
        let base = self.nodes;

        for i in start_idx..=end_idx {
            if *base.add(i) != 0 {
                return true;
            }
        }
        false
    }
}

pub struct Segment {
    next: *mut Segment,
    va_start: usize,
    va_end: usize,
    pub map: AtomicPtr<AtomicU64>,
    hint: AtomicUsize,
    pub map_len: usize,
    pub full: AtomicBool,
    failed_trys: AtomicU8,
}

pub struct VaBitmap {
    map: AtomicPtr<Segment>,
    latest_segment: AtomicPtr<Segment>,
    lock: SerialLock,
    radix_tree: SegmentIndex,
}

impl VaBitmap {
    pub const fn new() -> Self {
        Self {
            map: AtomicPtr::new(null_mut()),
            latest_segment: AtomicPtr::new(null_mut()),
            lock: SerialLock::new(),
            radix_tree: SegmentIndex {
                nodes: const { null_mut() },
            },
        }
    }

    #[inline(always)]
    pub unsafe fn is_ours(&self, addr: usize) -> bool {
        if unlikely(self.radix_tree.nodes.is_null()) {
            return false;
        }
        let segment = self.radix_tree.get_segment(addr);
        if unlikely(segment.is_null()) {
            return false;
        }
        let s = &*segment;
        addr >= s.va_start && addr < s.va_end
    }

    pub unsafe fn grow(&mut self) -> Option<*mut Segment> {
        let _guard = self.lock.lock();

        if unlikely(self.radix_tree.nodes.is_null()) {
            ONCE_PROTECTION.call_once(|| {
                self.radix_tree = SegmentIndex::new();
            });
        }

        let (user_va, end, total_size) = get_va_from_kernel();

        if user_va.is_null() {
            self.lock.unlock();
            return None;
        }

        if self
            .radix_tree
            .check_collision(user_va as usize, total_size)
        {
            self.lock.unlock();
            return None;
        }

        let bit_count = total_size / BLOCK_SIZE;
        let map_len = (bit_count + 63) / 64;
        let map_bytes = map_len * size_of::<u64>();

        let map_raw = match mmap_memory(
            null_mut(),
            map_bytes,
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        ) {
            Ok(ptr) => ptr,
            Err(_) => {
                self.lock.unlock();
                return None;
            }
        };

        let seg_ptr = match mmap_memory(
            null_mut(),
            size_of::<Segment>(),
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        ) {
            Ok(ptr) => ptr as *mut Segment,
            Err(_) => {
                self.lock.unlock();
                return None;
            }
        };

        let seed = {
            let mut v = user_va as usize;
            v ^= v >> 33;
            v ^= v << 17;
            v ^= v >> 7;
            v
        };

        let old_head = self.map.load(Ordering::Relaxed);
        write(
            seg_ptr,
            Segment {
                next: old_head,
                va_start: user_va as usize,
                va_end: end,
                map: AtomicPtr::new(map_raw as *mut AtomicU64),
                hint: AtomicUsize::new(seed % map_len),
                map_len,
                full: AtomicBool::new(false),
                failed_trys: AtomicU8::new(0),
            },
        );

        self.radix_tree
            .set_range(user_va as usize, total_size, seg_ptr);
        self.map.store(seg_ptr, Ordering::Release);
        self.lock.unlock();

        Some(seg_ptr)
    }

    #[inline(always)]
    pub unsafe fn alloc(&mut self, size: usize) -> Option<usize> {
        if unlikely(size == 0) {
            return None;
        }

        let needed = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;
        let hint_ptr = self.latest_segment.load(Ordering::Relaxed);

        if likely(!hint_ptr.is_null()) {
            let segment = &*hint_ptr;
            let res = if needed == 1 {
                segment.alloc_single()
            } else {
                segment.alloc_multi(needed)
            };
            if likely(res.is_some()) {
                return res;
            }
        }

        let mut curr = self.map.load(Ordering::Acquire);
        if unlikely(curr.is_null()) {
            ONCE.call_once(|| {
                match self.grow() {
                    Some(new) => curr = new,
                    None => OxidallocError::VAIinitFailed.log_and_abort(
                        null_mut(),
                        "VA initialization failed during allocator start",
                        None,
                    ),
                };
            });
            if curr.is_null() {
                curr = self.map.load(Ordering::Acquire);
            }
        }

        while !curr.is_null() {
            let segment = &*curr;
            if segment.full.load(Ordering::Relaxed) {
                curr = (*curr).next;
                continue;
            }

            let res = if needed == 1 {
                segment.alloc_single()
            } else {
                segment.alloc_multi(needed)
            };

            if res.is_some() {
                self.latest_segment.store(curr, Ordering::Release);
                return res;
            }
            let try_count = segment.failed_trys.fetch_add(1, Ordering::Relaxed);
            if try_count > 2 {
                segment.full.store(true, Ordering::Relaxed);
            }

            curr = (*curr).next;
        }

        let mut tried = 0usize;
        let mut new_seg_ptr = None;

        while tried < 10 {
            if let Some(seg) = self.grow() {
                new_seg_ptr = Some(seg);
                break;
            }
            tried += 1;
            std::hint::spin_loop();
        }

        let seg_ptr = new_seg_ptr?;
        self.latest_segment.store(seg_ptr, Ordering::Release);

        let new_seg = &*seg_ptr;
        if needed == 1 {
            new_seg.alloc_single()
        } else {
            new_seg.alloc_multi(needed)
        }
    }

    pub unsafe fn free(&self, addr: usize, size: usize) {
        if unlikely(addr == 0 || size == 0) {
            return;
        }

        let segment = self.radix_tree.get_segment(addr);
        if likely(!segment.is_null()) {
            let s = &*segment;
            if likely(addr >= s.va_start && addr < s.va_end) {
                s.free(addr, size);
                return;
            }
        }

        let mut curr = self.map.load(Ordering::Acquire);
        while !curr.is_null() {
            let s = &*curr;
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
        let segment = self.radix_tree.get_segment(addr);
        if likely(!segment.is_null()) {
            let s = &*segment;
            if likely(addr >= s.va_start && addr < s.va_end) {
                return s.realloc_inplace(addr, old_size, new_size);
            }
        }

        let mut curr = self.map.load(Ordering::Acquire);
        while !curr.is_null() {
            let s = &*curr;
            if addr >= s.va_start && addr < s.va_end {
                return s.realloc_inplace(addr, old_size, new_size);
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
    const fn max_bits(&self) -> usize {
        (self.va_end - self.va_start) / BLOCK_SIZE
    }

    #[inline(always)]
    fn alloc_single(&self) -> Option<usize> {
        let map = unsafe { self.get_map() };
        let total_bits = self.max_bits();
        if unlikely(total_bits == 0) {
            return None;
        }

        let chunks = (total_bits + 63) / 64;
        let h = self.hint.load(Ordering::Relaxed);
        let r = unsafe { alloc_random() };
        let rand_blocks = (r) & (MAX_RANDOM_BLOCKS - 1);
        let start_chunk = (h * 64 + rand_blocks) / 64 % chunks;
        let last_valid_bits = total_bits % 64;

        for (range_start, range_end) in [(start_chunk, chunks), (0, start_chunk)] {
            for i in range_start..range_end {
                let mut chunk = map[i].load(Ordering::Relaxed);

                if chunk == u64::MAX {
                    continue;
                }

                if i == chunks - 1 && last_valid_bits != 0 {
                    chunk |= !((1u64 << last_valid_bits) - 1);
                    if chunk == u64::MAX {
                        continue;
                    }
                }

                while chunk != u64::MAX {
                    let bit = (!chunk).trailing_zeros();
                    let mask = 1u64 << bit;

                    if (map[i].fetch_or(mask, Ordering::Acquire) & mask) == 0 {
                        self.hint.store(i, Ordering::Relaxed);

                        let global_idx = (i * 64) + bit as usize;
                        if unlikely(global_idx >= total_bits) {
                            map[i].fetch_and(!mask, Ordering::Release);
                            break;
                        }

                        return Some(self.va_start + (global_idx * BLOCK_SIZE));
                    }
                    chunk |= mask;
                }
            }
        }
        None
    }

    #[inline(always)]
    fn alloc_multi(&self, count: usize) -> Option<usize> {
        let map = unsafe { self.get_map() };
        let total_bits = self.max_bits();
        if unlikely(total_bits == 0 || count > total_bits) {
            return None;
        }

        let h = self.hint.load(Ordering::Relaxed);
        let r = unsafe { alloc_random() };
        let rand_bits = (r) & (MAX_RANDOM_BLOCKS - 1);
        let start_bit = (h * 64 + rand_bits) % total_bits;

        for (range_start, range_end) in [(start_bit, total_bits), (0, start_bit)] {
            let mut current_run = 0usize;
            let mut run_start = 0usize;
            let mut global_bit = range_start;

            while global_bit < range_end {
                let chunk_idx = global_bit / 64;
                let bit_in_chunk = global_bit % 64;

                let chunk = map[chunk_idx].load(Ordering::Relaxed);

                if bit_in_chunk == 0 && chunk == 0 {
                    let remaining_in_range = range_end - global_bit;
                    let skip = 64.min(remaining_in_range);

                    if current_run == 0 {
                        run_start = global_bit;
                    }
                    current_run += skip;

                    if current_run >= count {
                        if self.try_claim(run_start, count) {
                            self.hint.store(run_start / 64, Ordering::Relaxed);
                            return Some(self.va_start + (run_start * BLOCK_SIZE));
                        }
                        current_run = 0;
                        global_bit += 1;
                        continue;
                    }

                    global_bit += skip;
                    continue;
                }

                if chunk == u64::MAX {
                    current_run = 0;
                    global_bit = (chunk_idx + 1) * 64;
                    continue;
                }

                if (chunk & (1u64 << bit_in_chunk)) != 0 {
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
                global_bit += 1;
            }

            if range_start == 0 && range_end == start_bit && current_run > 0 {
                let carry_start = run_start;
                let carry_len = current_run;
                let mut extend = 0usize;
                let need = count.saturating_sub(carry_len);

                if need > 0 {
                    let mut b = start_bit;
                    while b < total_bits && extend < need {
                        let chunk = map[b / 64].load(Ordering::Relaxed);
                        if (chunk & (1u64 << (b % 64))) != 0 {
                            break;
                        }
                        extend += 1;
                        b += 1;
                    }
                }

                if carry_len + extend >= count && self.try_claim(carry_start, count) {
                    self.hint.store(carry_start / 64, Ordering::Relaxed);
                    return Some(self.va_start + (carry_start * BLOCK_SIZE));
                }
            }
        }
        None
    }

    #[inline(always)]
    fn try_claim(&self, start_idx: usize, count: usize) -> bool {
        let map = unsafe { self.get_map() };
        let total_bits = self.max_bits();

        if unlikely(start_idx + count > total_bits) {
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

    #[inline(always)]
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
        if unlikely(addr < start || addr >= end) {
            return;
        }

        let total_bits = self.max_bits();
        if unlikely(total_bits == 0) {
            return;
        }

        let offset = addr - start;
        let start_idx = offset / BLOCK_SIZE;
        if unlikely(start_idx >= total_bits) {
            return;
        }

        let mut count = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;
        if start_idx + count > total_bits {
            count = total_bits - start_idx;
        }

        self.rollback(start_idx, count);

        let chunk = start_idx / 64;
        let current_hint = self.hint.load(Ordering::Relaxed);
        if chunk < current_hint {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_crossing() {
        unsafe {
            let size_1 = [1024 * 1024 * 1024 * 3, 1024 * 1024 * 1024 * 2];
            let mut adresses = Vec::new();

            for i in 0..256 {
                let addr = VA_MAP.alloc(size_1[i % 2]).expect(&format!(
                    "Failed to allocate {}b, loop: {}",
                    size_1[i % 2],
                    i
                ));

                assert!(
                    VA_MAP.is_ours(addr),
                    "Address is not ours, size: {}",
                    size_1[i % 2]
                );

                adresses.push([addr, size_1[i % 2]]);
            }

            eprintln!("Cleaning up...");
            for addr in adresses {
                VA_MAP.free(addr[0], addr[1]);
            }
        }
    }
}
