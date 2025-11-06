use std::{
    os::raw::c_void,
    sync::atomic::{AtomicBool, Ordering},
};

use libc::madvise;

use crate::{HEADER_SIZE, ITERATIONS, TOTAL_ALLOCATED, TOTAL_USED, global::GlobalHandler};

pub static IS_TRIMMING: AtomicBool = AtomicBool::new(false);

pub struct Trimmer;

impl Trimmer {
    pub fn determine(&self) {
        let used = TOTAL_USED.load(Ordering::Relaxed);
        let allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);

        if allocated == 0 {
            return;
        }

        let usage_percent = (used * 100) / allocated;

        if usage_percent > 50 {
            return;
        }

        if IS_TRIMMING.load(Ordering::Relaxed) {
            return;
        }

        self.trim();
    }

    fn trim(&self) {
        IS_TRIMMING.store(true, Ordering::Relaxed);

        for class in 0..20 {
            let mut list =
                unsafe { GlobalHandler.pop_batch_from_global(class, ITERATIONS[class] / 2) };

            unsafe {
                for _ in 0..ITERATIONS[class] / 2 {
                    if list.is_null() {
                        break;
                    }

                    let size = (*list).size;

                    madvise(
                        list as *mut c_void,
                        size as usize + HEADER_SIZE,
                        libc::MADV_FREE,
                    );

                    TOTAL_ALLOCATED.fetch_sub(size as usize + HEADER_SIZE, Ordering::Relaxed);

                    list = (*list).next;
                }
            }
        }

        IS_TRIMMING.store(false, Ordering::Relaxed);
    }
}
