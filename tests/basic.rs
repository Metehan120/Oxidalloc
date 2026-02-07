use std::{
    hint::black_box,
    os::raw::{c_int, c_void},
    ptr::write_bytes,
};

unsafe extern "C" {
    pub fn malloc(size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
    pub fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    pub fn posix_memalign(memptr: *mut *mut c_void, alignment: usize, size: usize) -> c_int;
    pub fn malloc_usable_size(ptr: *mut c_void) -> usize;
    pub fn valloc(size: usize) -> *mut c_void;
    pub fn pvalloc(size: usize) -> *mut c_void;
}

#[test]
fn test_valloc_and_pvalloc() {
    unsafe {
        let ptr1 = valloc(100);
        assert!(!ptr1.is_null());
        assert_eq!((ptr1 as usize) % 4096, 0);
        free(ptr1);

        let ptr2 = pvalloc(100);
        assert!(!ptr2.is_null());
        assert_eq!((ptr2 as usize) % 4096, 0);
        let usable2 = malloc_usable_size(ptr2);
        assert!(usable2 >= 4096);
        free(ptr2);

        let ptr3 = pvalloc(5000);
        assert!(!ptr3.is_null());
        assert_eq!((ptr3 as usize) % 4096, 0);
        let usable3 = malloc_usable_size(ptr3);
        assert!(usable3 >= 8192);
        free(ptr3);
    }
}

#[test]
fn smoke_global_reuse() {
    unsafe {
        use std::thread;

        let worker = thread::spawn(|| {
            for _ in 0..10_000 {
                let ptr = malloc(128);
                free(ptr);
            }
        });
        worker.join().unwrap();

        for _ in 0..1000 {
            let ptr = malloc(128);
            assert!(!ptr.is_null());
            free(ptr);
        }
    }
}

#[test]
fn realloc_handles_posix_memalign_pointer() {
    unsafe {
        let mut ptr: *mut c_void = std::ptr::null_mut();
        assert_eq!(posix_memalign(&mut ptr, 64, 100), 0);
        assert!(!ptr.is_null());
        assert_eq!((ptr as usize) % 64, 0);

        let initial = std::slice::from_raw_parts_mut(ptr as *mut u8, 100);
        for (i, byte) in initial.iter_mut().enumerate() {
            *byte = (i as u8).wrapping_mul(3).wrapping_add(1);
        }

        let old_usable = malloc_usable_size(ptr) as usize;
        assert!(old_usable >= 100);

        let new_ptr = realloc(ptr, old_usable + 32);
        assert!(!new_ptr.is_null());

        let after = std::slice::from_raw_parts(new_ptr as *const u8, 100);
        for i in 0..100 {
            assert_eq!(after[i], (i as u8).wrapping_mul(3).wrapping_add(1));
        }

        free(new_ptr);
    }
}

#[test]
fn testing_out_of_class_reallocs() {
    unsafe {
        let ptr = malloc(1024 * 1024 * 4);
        assert!(!ptr.is_null(), "malloc failed");
        let start = std::time::Instant::now();
        let new_ptr = black_box(realloc(ptr, 1024 * 1024 * 8));
        let elapsed = start.elapsed().as_nanos();
        println!("Out of class realloc take (4mb -> 8mb) {}", elapsed);
        assert!(!new_ptr.is_null(), "realloc failed");
        free(new_ptr);
    }
}

#[test]
fn test_big_reallocs_1mb() {
    unsafe {
        let ptr = malloc(1024 * 1024);
        assert!(!ptr.is_null(), "malloc failed");
        black_box(write_bytes(ptr, 1, 1024 * 1024));
        let start = std::time::Instant::now();
        let new_ptr = black_box(realloc(ptr, 1024 * 1024 * 2));
        let elapsed = start.elapsed().as_nanos();
        assert!(!new_ptr.is_null(), "realloc failed");
        black_box(write_bytes(new_ptr, 2, 1024 * 1024 * 2));
        println!("Big realloc take (1mb -> 2mb) {}", elapsed);
        free(new_ptr);
    }
}

#[test]
fn test_realloc_shrink() {
    unsafe {
        let ptr = malloc(1024 * 1024 * 2);
        assert!(!ptr.is_null(), "malloc failed");
        black_box(write_bytes(ptr, 1, 1024 * 1024 * 2));
        let start = std::time::Instant::now();
        let new_ptr = black_box(realloc(ptr, 1024 * 1024));
        let elapsed = start.elapsed().as_nanos();
        assert!(!new_ptr.is_null(), "realloc failed");
        black_box(write_bytes(new_ptr, 1, 1024 * 1024));
        let usable_size = malloc_usable_size(new_ptr);
        println!(
            "Big realloc shrink take (2mb ->1mb) {}, usable size {}",
            elapsed, usable_size
        );
        free(new_ptr);
    }
}
