use std::ffi::c_void;
use std::os::raw::c_int;

// Import the symbols we expect Oxidalloc to export
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn calloc(nmemb: usize, size: usize) -> *mut c_void;
    fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    fn posix_memalign(memptr: *mut *mut c_void, alignment: usize, size: usize) -> c_int;
    fn memalign(alignment: usize, size: usize) -> *mut c_void;
    fn aligned_alloc(alignment: usize, size: usize) -> *mut c_void;
    fn valloc(size: usize) -> *mut c_void;
    fn malloc_usable_size(ptr: *mut c_void) -> usize;
}

#[test]
fn test_malloc_free() {
    unsafe {
        let size = 100;
        let ptr = malloc(size);
        assert!(!ptr.is_null(), "malloc returned null");

        let slice = std::slice::from_raw_parts_mut(ptr as *mut u8, size);
        for i in 0..size {
            slice[i] = 0xAA;
        }

        assert_eq!(slice[0], 0xAA);
        assert_eq!(slice[size - 1], 0xAA);

        let usable = malloc_usable_size(ptr);
        assert!(
            usable >= size,
            "malloc_usable_size {} < requested {}",
            usable,
            size
        );

        free(ptr);
    }
}

#[test]
fn test_calloc() {
    unsafe {
        let nmemb = 10;
        let size = 100;
        let ptr = calloc(nmemb, size);
        assert!(!ptr.is_null(), "calloc returned null");

        let total_size = nmemb * size;
        let slice = std::slice::from_raw_parts(ptr as *mut u8, total_size);

        for byte in slice {
            assert_eq!(*byte, 0, "calloc memory not zeroed");
        }

        free(ptr);
    }
}

#[test]
fn test_realloc() {
    unsafe {
        let size = 64;
        let ptr = malloc(size);
        assert!(!ptr.is_null());

        let slice = std::slice::from_raw_parts_mut(ptr as *mut u8, size);
        for i in 0..size {
            slice[i] = (i % 255) as u8;
        }

        let new_size = 128;
        let new_ptr = realloc(ptr, new_size);
        assert!(!new_ptr.is_null(), "realloc returned null");

        let new_slice = std::slice::from_raw_parts(new_ptr as *mut u8, new_size);

        for i in 0..size {
            assert_eq!(
                new_slice[i],
                (i % 255) as u8,
                "realloc failed to preserve data at index {}",
                i
            );
        }

        free(new_ptr);
    }
}

#[test]
fn test_posix_memalign() {
    unsafe {
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let align = 256;
        let size = 100;

        let res = posix_memalign(&mut ptr, align, size);
        assert_eq!(res, 0, "posix_memalign failed");
        assert!(!ptr.is_null());
        assert_eq!(
            ptr as usize % align,
            0,
            "posix_memalign returned unaligned address"
        );

        free(ptr);
    }
}

#[test]
fn test_memalign() {
    unsafe {
        let align = 512;
        let size = 1024;
        let ptr = memalign(align, size);
        assert!(!ptr.is_null());
        assert_eq!(
            ptr as usize % align,
            0,
            "memalign returned unaligned address"
        );
        free(ptr);
    }
}

#[test]
fn test_aligned_alloc() {
    unsafe {
        let align = 128;
        let size = 256;
        let ptr = aligned_alloc(align, size);
        assert!(!ptr.is_null());
        assert_eq!(
            ptr as usize % align,
            0,
            "aligned_alloc returned unaligned address"
        );
        free(ptr);
    }
}

#[test]
fn test_valloc() {
    unsafe {
        let size = 100;
        let ptr = valloc(size);
        assert!(!ptr.is_null());
        assert_eq!(
            ptr as usize % 4096,
            0,
            "valloc returned unaligned address (expected 4096)"
        );
        free(ptr);
    }
}
