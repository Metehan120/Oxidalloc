use crate::va::bitmap::VA_MAP;

pub mod bitmap;
pub mod bootstrap;

pub const fn align_to(size: usize, align: usize) -> usize {
    let al = align - 1;
    (size + al) & !al
}

#[inline(always)]
pub fn is_ours(addr: usize) -> bool {
    if addr % 8 != 0 {
        return false;
    }
    unsafe { VA_MAP.is_ours(addr) }
}
