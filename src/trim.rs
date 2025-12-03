use std::{
    os::raw::c_void,
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};

use libc::{MADV_FREE, madvise};

use crate::{
    OX_CURRENT_STAMP,
    global::{GLOBAL_USAGE, GlobalHandler},
    internals::ITERATIONS,
    thread_local::ThreadLocalEngine,
};

pub static IS_TRIMMING: AtomicBool = AtomicBool::new(false);

// TODO: Add freeing Memory to the OS logic
pub struct Trim;

impl Trim {
    pub fn trim(&self, cache: &ThreadLocalEngine) {
        unsafe {
            if IS_TRIMMING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                return;
            }

            for class in (0..cache.cache.len()).rev() {
                let mut _total_trimmed = 0;

                for _ in 0..cache.usages[class].load(Ordering::Relaxed) / 2 {
                    let cache_mem = cache.pop_from_thread(class);

                    if cache_mem.is_null() {
                        break;
                    }

                    _total_trimmed += 1;

                    (*cache_mem).next = null_mut();

                    let time = OX_CURRENT_STAMP
                        .load(Ordering::Relaxed)
                        .saturating_sub((*cache_mem).life_time);

                    let usage = cache.usages[class].load(Ordering::Relaxed);

                    // WARNING: TEST ONLY
                    #[cfg(debug_assertions)]
                    if time > 2 && _total_trimmed < ITERATIONS[class] && usage > ITERATIONS[class] {
                        cache.usages[class].fetch_sub(1, Ordering::Relaxed);

                        madvise(
                            cache_mem as *mut c_void,
                            (*cache_mem).size as usize,
                            MADV_FREE,
                        );

                        continue;
                    }

                    if time > 1 {
                        cache.usages[class].fetch_sub(1, Ordering::Relaxed);
                        GLOBAL_USAGE[class].fetch_add(1, Ordering::Relaxed);

                        (*cache_mem).life_time = 0;

                        GlobalHandler.push_to_global(class, cache_mem, cache_mem);
                        continue;
                    }

                    cache.push_to_thread(class, cache_mem);
                }
            }

            IS_TRIMMING.store(false, Ordering::Release);
        }
    }
}
