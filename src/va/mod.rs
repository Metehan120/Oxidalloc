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

#[cfg(test)]
mod tests {
    use std::{hint::black_box, time::Instant};

    use super::*;

    #[test]
    fn is_ours_speed_test() {
        unsafe {
            const N: usize = 10_000_000;
            let ptr = VA_MAP.alloc(1024 * 1024 * 1024).unwrap();

            let start = Instant::now();
            for _ in 0..N {
                black_box(is_ours(ptr as usize));
            }
            let end = Instant::now();
            let ns = end.duration_since(start).as_nanos() as f64 / N as f64;
            println!("Get speed: {:.2} ns/op", ns);
        }
    }
}
