use std::{
    os::raw::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use libc::{__errno_location, madvise};

use crate::{
    GLOBAL_TRIM_INTERVAL, HEADER_SIZE, LOCAL_TRIM_INTERVAL, OxHeader, OxidallocError,
    TOTAL_ALLOCATED, TOTAL_IN_USE, TOTAL_OPS,
    internals::{AllocationHelper, MAGIC, VA_END, VA_MAP, VA_START, align},
    thread_local::ThreadLocalEngine,
    trim::Trim,
};

#[inline(always)]
pub fn is_ours(ptr: *mut c_void) -> bool {
    let start = VA_START.load(Ordering::Acquire);
    let end = VA_END.load(Ordering::Acquire);

    if start == 0 || end == 0 || start >= end {
        return false;
    }

    let addr = ptr as usize;
    addr >= start && addr < end
}

const OFFSET_SIZE: usize = size_of::<usize>();

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    unsafe {
        if ptr.is_null() {
            return;
        }

        if !is_ours(ptr) {
            return;
        }

        let mut header_search_ptr = ptr;
        let presumed_original_ptr_loc = (ptr as usize).wrapping_sub(OFFSET_SIZE) as *mut usize;
        let tagged = *presumed_original_ptr_loc;

        if (tagged & 1) != 0 {
            let presumed_original_ptr = (tagged & !1) as *mut c_void;
            if is_ours(presumed_original_ptr) {
                header_search_ptr = presumed_original_ptr;
            }
        }

        let header_addr = (header_search_ptr as usize).wrapping_sub(HEADER_SIZE);
        let header = header_addr as *mut OxHeader;

        if !is_ours(header as *mut c_void) {
            return;
        }

        if header_addr % 4096 > 4096 - HEADER_SIZE {
            return;
        }

        let magic_val = std::ptr::read_volatile(&(*header).magic);
        let size = std::ptr::read_volatile(&(*header).size);

        if magic_val != MAGIC && magic_val != 0 {
            OxidallocError::MemoryCorruption
                .log_and_abort(header as *mut c_void, "Possibly Double Free");
        }

        if (*header).in_use.load(Ordering::Relaxed) == 0 {
            OxidallocError::DoubleFree
                .log_and_abort(header as *mut c_void, "Pointer is tagged as in_use");
        }

        let thread = ThreadLocalEngine::get_or_init();
        let class = match AllocationHelper.match_size_class(size as usize) {
            Some(class) => class,
            None => {
                if size == 0
                    || size as usize
                        > (VA_END.load(Ordering::Acquire) - VA_START.load(Ordering::Acquire))
                {
                    eprintln!("Unexpected size in free: size={}, ptr={:p}", size, header);
                    return;
                }

                (*header).magic = 0;
                (*header).change_in_use_state(header as *mut c_void);

                let aligned = align(size as usize + HEADER_SIZE);

                if madvise(header as *mut c_void, aligned, libc::MADV_DONTNEED) != 0 {
                    eprintln!(
                        "Madvise Failed, memory leaked. size={}, aligned={}, errno={}",
                        size,
                        aligned,
                        *__errno_location()
                    );
                    return;
                }

                VA_MAP.free(header as usize, aligned);

                trim(thread);
                return;
            }
        };

        (*header).magic = 0;
        (*header).change_in_use_state(header as *mut c_void);
        thread.push_to_thread(class, header);
        thread.usages[class].fetch_add(1, Ordering::Relaxed);

        TOTAL_IN_USE.fetch_sub(1, Ordering::Relaxed);

        trim(thread);
    }
}

static LAST_PRESSURE_CHECK: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
pub fn trim(engine: &ThreadLocalEngine) {
    let total = TOTAL_OPS.load(Ordering::Relaxed);

    let total_allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);
    let total_in_use = TOTAL_IN_USE.load(Ordering::Relaxed);

    if total % 55000 == 0 {
        let pressure = check_memory_pressure();
        LAST_PRESSURE_CHECK.store(pressure, Ordering::Relaxed);
    }

    let in_use_percentage = if total_allocated > 0 {
        (total_in_use * 100) / total_allocated
    } else {
        0
    };

    let pressure = LAST_PRESSURE_CHECK.load(Ordering::Relaxed);

    if total % GLOBAL_TRIM_INTERVAL.load(Ordering::Relaxed) == 0
        || pressure > 85
        || in_use_percentage < 35
    {
        Trim.trim_global();
    } else if total % LOCAL_TRIM_INTERVAL.load(Ordering::Relaxed) == 0
        || pressure > 75
        || in_use_percentage < 50
    {
        Trim.trim(engine);
    }
}

fn check_memory_pressure() -> usize {
    unsafe {
        let mut info: libc::sysinfo = std::mem::zeroed();

        if libc::sysinfo(&mut info) != 0 {
            return 50;
        }

        let total_ram = info.totalram as usize;
        let free_ram = info.freeram as usize;
        let total_swap = info.totalswap as usize;
        let free_swap = info.freeswap as usize;

        let total_available = free_ram + free_swap;
        let total_memory = total_ram + total_swap;

        if total_memory == 0 {
            return 50;
        }

        let used = total_memory.saturating_sub(total_available);
        (used * 100) / total_memory
    }
}
