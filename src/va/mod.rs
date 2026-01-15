use std::sync::atomic::Ordering;

use crate::va::bootstrap::{VA_END, VA_START};

pub mod bitmap;
pub mod bootstrap;

pub fn align_to(size: usize, align: usize) -> usize {
    let al = align - 1;
    (size + al) & !al
}

#[inline(always)]
pub fn is_ours(addr: usize) -> bool {
    let start = VA_START.load(Ordering::Relaxed);
    let end = VA_END.load(Ordering::Relaxed);
    if addr == 0 || addr < start || addr >= end {
        return false;
    }
    addr >= start && addr < end
}
