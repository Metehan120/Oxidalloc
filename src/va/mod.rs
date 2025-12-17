pub mod bitmap;
pub mod bootstrap;

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
        if addr % 8 != 0 {
            return false;
        }
        addr >= start && addr < end
    }
}

pub fn align_to(size: usize, align: usize) -> usize {
    let al = align - 1;
    (size + al) & !al
}
