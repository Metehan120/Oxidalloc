#![no_main]

use std::os::raw::c_void;

use libc::size_t;
use libfuzzer_sys::fuzz_target;
use rand::random_range;

unsafe extern "C" {
    fn malloc(size: size_t) -> *mut c_void;
    fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn calloc(nmemb: size_t, size: size_t) -> *mut c_void;
}

// This test correctness of allocator not speed, **you have to LD_PRELOAD to see actual test**
fuzz_target!(|data: &[u8]| {
    unsafe {
        let mut len = 0usize;
        let mut idx = 0usize;

        while idx < data.len() {
            len += 1;
            if len > (1 << 20) {
                break;
            }

            if len % 128 == 0 && idx % 2 == 0 {
                let mut ptr = malloc(len);
                ptr = realloc(ptr, 1024);
                if !ptr.is_null() {
                    free(ptr);
                }
            } else {
                let ptr = malloc(len * idx);
                if !ptr.is_null() {
                    free(ptr);
                }
            }

            if len % 48 == 0 {
                let ptr = calloc(len, len);
                if !ptr.is_null() {
                    free(ptr);
                }
            } else {
                let ptr = calloc(len, len);
                let ptr = realloc(ptr, len);
                if !ptr.is_null() {
                    free(ptr);
                }
            }

            if len % 99 == 0 {
                let size = random_range(0..4096);
                let random = malloc(size);
                free(random as *mut c_void);
            }

            if len > data[idx] as usize {
                idx += 1;
            }
        }
    }
});
