use crate::va::bootstrap::{VA_END, VA_START};
use libc::size_t;
use std::sync::atomic::Ordering;

pub mod bitmap;
pub mod bootstrap;

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn vaReserveSize() -> size_t {
    VA_END.load(Ordering::Relaxed) - VA_START.load(Ordering::Relaxed)
}

pub mod va_helper {
    use std::sync::atomic::Ordering;

    use crate::va::bootstrap::{VA_END, VA_START};

    #[inline(always)]
    pub fn is_ours(addr: usize) -> bool {
        let start = VA_START.load(Ordering::Acquire);
        let end = VA_END.load(Ordering::Acquire);
        if start == 0 || end == 0 || start >= end {
            return false;
        }
        addr >= start && addr < end
    }
}
