use std::hint::black_box;

unsafe extern "C" {
    fn malloc(size: libc::size_t) -> *mut libc::c_void;
    fn free(ptr: *mut libc::c_void);
}

#[test]
fn bitmap_stress_test() {
    eprintln!("(bitmap_stress_test) Assuming 1 Thread call only");
    eprintln!(
        "Oxidalloc doesnt support Big Block Caching, expect slower performance on this benchmark-only;
        under real workload shouldnt be than different any other malloc like jemalloc, tcmalloc, glibc, etc."
    );
    let start = std::time::Instant::now();
    for _ in 0..2048 {
        let malloc = unsafe { black_box(malloc(1024 * 1024 * 1024)) };
        assert!(!malloc.is_null());
        unsafe { black_box(free(malloc)) };
    }
    let elapsed = start.elapsed();
    println!(
        "General Test took, including syscall overhead on free {}ms",
        elapsed.as_millis()
    );
}
