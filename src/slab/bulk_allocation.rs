use std::{os::raw::c_void, ptr::null_mut, sync::atomic::Ordering};

use rustix::mm::{Advice, MapFlags, ProtFlags, madvise, mmap_anonymous};

use crate::{
    Err, HEADER_SIZE, MAGIC, OxHeader, TOTAL_ALLOCATED,
    slab::{ITERATIONS, SIZE_CLASSES, thread_local::ThreadLocalEngine},
    va::{align_to, bitmap::VA_MAP},
};
use std::mem::align_of;

pub fn bulk_fill(thread: &ThreadLocalEngine, class: usize) -> Result<(), Err> {
    unsafe {
        let payload_size = SIZE_CLASSES[class];
        let block_size = align_to(payload_size + HEADER_SIZE, align_of::<OxHeader>());
        let total = align_to((block_size * ITERATIONS[class]) + 4096, 4096);

        let hint = match VA_MAP.alloc(total) {
            Some(size) => size,
            None => return Err(Err::OutOfReservation),
        };

        let mem = match mmap_anonymous(
            hint as *mut c_void,
            total,
            ProtFlags::WRITE | ProtFlags::READ,
            MapFlags::PRIVATE | MapFlags::FIXED,
        ) {
            Ok(mem) => mem,
            Err(_) => return Err(Err::OutOfMemory),
        };

        let used_end = (mem as usize) + (block_size * ITERATIONS[class]);
        let page_end = (used_end + 4096 - 1) & !(4096 - 1);
        let slack = page_end - used_end;
        let drop_len = slack & !(4096 - 1);

        if drop_len > 0 {
            match madvise(mem, drop_len, Advice::LinuxDontNeed) {
                Ok(_) => (),
                Err(_) => (),
            };
        }

        if class > 14 {
            match madvise(mem, total, Advice::LinuxHugepage) {
                Ok(_) => (),
                Err(_) => (),
            };
        }

        let mut prev = null_mut();
        for i in (0..ITERATIONS[class]).rev() {
            let current_header = (mem as usize + i * block_size) as *mut OxHeader;
            (*current_header).next = prev;
            (*current_header).size = payload_size as u64;
            (*current_header).magic = MAGIC;
            (*current_header).in_use = 0;
            prev = current_header;
        }

        let mut tail = prev;
        for _ in 0..ITERATIONS[class] - 1 {
            tail = (*tail).next;
        }

        thread.push_to_thread_tailed(class, prev, tail);
        TOTAL_ALLOCATED.fetch_add(ITERATIONS[class], Ordering::Relaxed);

        Ok(())
    }
}
