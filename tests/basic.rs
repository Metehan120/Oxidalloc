use std::os::raw::{c_int, c_void};

unsafe extern "C" {
    pub fn malloc(size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
    pub fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    pub fn posix_memalign(memptr: *mut *mut c_void, alignment: usize, size: usize) -> c_int;
    pub fn malloc_usable_size(ptr: *mut c_void) -> usize;
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
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
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
