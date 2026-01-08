use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::va::bootstrap::{VA_END, VA_START};

pub const BLOCK_SIZE: usize = 4096;
pub static BITMAP_LEN: usize = 65_536 * 16;

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

    #[inline(always)]
    fn max_bits(&self) -> usize {
        let start = VA_START.load(Ordering::Relaxed);
        let end = VA_END.load(Ordering::Relaxed);

        if start == 0 || end <= start {
            return 0;
        }

        let bytes = end - start;
        let blocks = bytes / BLOCK_SIZE;
        blocks.min(BITMAP_LEN * 64)
    }

    pub fn alloc(&self, size: usize) -> Option<usize> {
        if size == 0 {
            return None;
        }

        let needed = (size / BLOCK_SIZE) + usize::from(size % BLOCK_SIZE != 0);

        if needed == 1 {
            return self.alloc_single();
        }
        self.alloc_multi(needed)
    }

    fn alloc_single(&self) -> Option<usize> {
        let total_bits = self.max_bits();
        if total_bits == 0 {
            return None;
        }

        let chunks = (total_bits + 63) / 64;
        let start_chunk = self.hint.load(Ordering::Relaxed) % chunks;
        let last_valid_bits = total_bits % 64;

        for (range_start, range_end) in [(start_chunk, chunks), (0, start_chunk)] {
            for i in range_start..range_end {
                let mut chunk = self.map[i].load(Ordering::Relaxed);
                if i == chunks - 1 && last_valid_bits != 0 {
                    chunk |= !((1u64 << last_valid_bits) - 1);
                }

                if chunk == u64::MAX {
                    continue;
                }

                let bit = (!chunk).trailing_zeros();
                let mask = 1u64 << bit;

                if (self.map[i].fetch_or(mask, Ordering::Acquire) & mask) == 0 {
                    self.hint.store(i, Ordering::Relaxed);
                    let global_idx = (i * 64) + bit as usize;
                    if global_idx >= total_bits {
                        self.map[i].fetch_and(!mask, Ordering::Release);
                        continue;
                    }

                    let addr = VA_START.load(Ordering::Relaxed) + (global_idx * BLOCK_SIZE);
                    return Some(addr);
                }
            }
        }
        None
    }

    fn alloc_multi(&self, count: usize) -> Option<usize> {
        let total_bits = self.max_bits();
        if total_bits == 0 {
            return None;
        }

        if count > total_bits {
            return None;
        }

        let start_bit = self.hint.load(Ordering::Relaxed) * 64;
        let start_bit = if start_bit >= total_bits {
            0
        } else {
            start_bit
        };

        for (range_start, range_end) in [(start_bit, total_bits), (0, start_bit)] {
            let mut current_run = 0;
            let mut run_start = 0;

            for global_bit in range_start..range_end {
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
        }
        None
    }

    fn try_claim(&self, start_idx: usize, count: usize) -> bool {
        let total_bits = self.max_bits();
        if total_bits == 0 {
            return false;
        }

        let end = match start_idx.checked_add(count) {
            Some(end) => end,
            None => return false,
        };

        if start_idx >= total_bits || end > total_bits {
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
        let end = VA_END.load(Ordering::Relaxed);
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
}
