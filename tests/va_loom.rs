use loom::sync::atomic::{AtomicU64, Ordering};
use loom::thread;

pub struct LoomSegment {
    pub map: Vec<AtomicU64>,
    pub claim: Vec<AtomicU64>,
    pub map_len: usize,
}

impl LoomSegment {
    pub fn new(map_len: usize) -> Self {
        let mut map = Vec::with_capacity(map_len);
        let mut claim = Vec::with_capacity(map_len);
        for _ in 0..map_len {
            map.push(AtomicU64::new(0));
            claim.push(AtomicU64::new(0));
        }
        Self {
            map,
            claim,
            map_len,
        }
    }

    pub fn try_claim(&self, start_idx: usize, count: usize) -> bool {
        let mut bits_processed = 0;

        while bits_processed < count {
            let current_bit = start_idx + bits_processed;
            let chunk_idx = current_bit / 64;
            let shift = current_bit % 64;

            let bits_in_this_chunk = (64 - shift).min(count - bits_processed);
            let mask = if bits_in_this_chunk == 64 {
                u64::MAX
            } else {
                ((1u64 << bits_in_this_chunk) - 1) << shift
            };

            let mut claim_val = self.claim[chunk_idx].load(Ordering::Acquire);
            loop {
                let map_val = self.map[chunk_idx].load(Ordering::Acquire);
                if ((claim_val | map_val) & mask) != 0 {
                    self.rollback_claim(start_idx, bits_processed);
                    return false;
                }

                let next = claim_val | mask;
                match self.claim[chunk_idx].compare_exchange_weak(
                    claim_val,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // RE-CHECK MAP AFTER SUCCESSFUL CLAIM
                        // This prevents a race where another thread committed just before we claimed
                        let recheck_map = self.map[chunk_idx].load(Ordering::Acquire);
                        if (recheck_map & mask) != 0 {
                            // Already committed by someone else!
                            self.rollback_claim(start_idx, bits_processed + bits_in_this_chunk);
                            return false;
                        }
                        break;
                    }
                    Err(actual) => claim_val = actual,
                }
            }
            bits_processed += bits_in_this_chunk;
        }

        bits_processed = 0;
        while bits_processed < count {
            let current_bit = start_idx + bits_processed;
            let chunk_idx = current_bit / 64;
            let shift = current_bit % 64;
            let bits_in_this_chunk = (64 - shift).min(count - bits_processed);
            let mask = if bits_in_this_chunk == 64 {
                u64::MAX
            } else {
                ((1u64 << bits_in_this_chunk) - 1) << shift
            };

            self.map[chunk_idx].fetch_or(mask, Ordering::AcqRel);
            bits_processed += bits_in_this_chunk;
        }

        bits_processed = 0;
        while bits_processed < count {
            let current_bit = start_idx + bits_processed;
            let chunk_idx = current_bit / 64;
            let shift = current_bit % 64;
            let bits_in_this_chunk = (64 - shift).min(count - bits_processed);
            let mask = if bits_in_this_chunk == 64 {
                u64::MAX
            } else {
                ((1u64 << bits_in_this_chunk) - 1) << shift
            };

            self.claim[chunk_idx].fetch_and(!mask, Ordering::AcqRel);
            bits_processed += bits_in_this_chunk;
        }

        true
    }

    fn rollback_claim(&self, start_idx: usize, count: usize) {
        let mut bits_processed = 0;
        while bits_processed < count {
            let current_bit = start_idx + bits_processed;
            let chunk_idx = current_bit / 64;
            let shift = current_bit % 64;
            let bits_in_this_chunk = (64 - shift).min(count - bits_processed);
            let mask = if bits_in_this_chunk == 64 {
                u64::MAX
            } else {
                ((1u64 << bits_in_this_chunk) - 1) << shift
            };

            self.claim[chunk_idx].fetch_and(!mask, Ordering::AcqRel);
            bits_processed += bits_in_this_chunk;
        }
    }
}

#[test]
fn test_va_claim_race() {
    loom::model(|| {
        let segment = std::sync::Arc::new(LoomSegment::new(1));

        let s1 = segment.clone();
        let t1 = thread::spawn(move || s1.try_claim(0, 32));

        let s2 = segment.clone();
        let t2 = thread::spawn(move || s2.try_claim(0, 32));

        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();

        assert!(
            r1 ^ r2,
            "Both threads returned same result: r1={}, r2={}",
            r1,
            r2
        );

        let map_val = segment.map[0].load(Ordering::Acquire);
        assert_eq!(map_val, (1u64 << 32) - 1);

        let claim_val = segment.claim[0].load(Ordering::Acquire);
        assert_eq!(claim_val, 0);
    });
}

#[test]
fn test_va_multi_chunk_race() {
    loom::model(|| {
        let segment = std::sync::Arc::new(LoomSegment::new(2));

        let s1 = segment.clone();
        let t1 = thread::spawn(move || {
            s1.try_claim(48, 16) // bit 48..64 (chunk 0)
        });

        let s2 = segment.clone();
        let t2 = thread::spawn(move || {
            s2.try_claim(60, 8) // bit 60..68 (overlaps chunk 0 and 1)
        });

        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();

        assert!(
            r1 ^ r2,
            "Both threads returned same result: r1={}, r2={}",
            r1,
            r2
        );
    });
}
