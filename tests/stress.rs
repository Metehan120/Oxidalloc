use std::{hint::black_box, thread};

unsafe extern "C" {
    fn malloc(size: libc::size_t) -> *mut libc::c_void;
    fn free(ptr: *mut libc::c_void);
}

#[test]
fn stress_organized_thread_calls() {
    let num_thread = thread::available_parallelism().unwrap();
    let mut threads = Vec::new();

    for _ in 0..num_thread.get() {
        threads.push(std::thread::spawn(|| {
            let thread_id = thread::current().id();
            let call_needed = 1024;

            for _ in 0..1024 * call_needed {
                let ptr = unsafe { black_box(malloc(1024)) };
                assert!(!ptr.is_null());
                unsafe { black_box(free(ptr)) };
            }

            println!(
                "Organized Stress Test, ID: {:?}, Loop: {}, completed no failure",
                thread_id,
                1024 * call_needed
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn stress_thread_with_random_calls() {
    let num_thread = thread::available_parallelism().unwrap();
    let mut threads = Vec::new();

    for _ in 0..num_thread.get() {
        threads.push(std::thread::spawn(|| {
            let call_needed = rand::random_range(0..2048);
            let thread_id = thread::current().id();

            for _ in 0..1024 * call_needed {
                let ptr = unsafe { black_box(malloc(1024)) };
                assert!(!ptr.is_null());
                unsafe { black_box(free(ptr)) };
            }

            println!(
                "Async Stress Test, ID: {:?}, Loop: {}, completed no failure",
                thread_id,
                1024 * call_needed
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn stress_thread_call() {
    let num_thread = thread::available_parallelism().unwrap();
    let mut threads = Vec::new();

    for _ in 0..num_thread.get() {
        threads.push(std::thread::spawn(|| {
            let thread_id = thread::current().id();
            let call_needed = 1024;

            for _ in 0..1024 * call_needed {
                let ptr = unsafe { black_box(malloc(1024)) };
                assert!(!ptr.is_null());
                unsafe { black_box(free(ptr)) };
            }

            println!(
                "Stress Test, ID: {:?}, Loop: {}, completed no failure",
                thread_id,
                1024 * call_needed
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn stress_test_random_malloc_multithread() {
    let num_thread = thread::available_parallelism().unwrap();
    let mut threads = Vec::new();

    for _ in 0..num_thread.get() {
        threads.push(std::thread::spawn(|| {
            let thread_id = thread::current().id();
            let call_needed = 1024;

            for _ in 0..1024 * call_needed {
                let random = rand::random_range(0..1024 * 512);
                let ptr = unsafe { black_box(malloc(random)) };
                assert!(!ptr.is_null());
                unsafe { black_box(free(ptr)) };
            }

            println!(
                "Stress Test, ID: {:?}, Loop: {}, completed no failure",
                thread_id,
                1024 * call_needed
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn stress_thread_with_random_calls_and_random_mallocs() {
    let num_thread = thread::available_parallelism().unwrap();
    let mut threads = Vec::new();

    for _ in 0..num_thread.get() {
        threads.push(std::thread::spawn(|| {
            let call_needed = rand::random_range(0..2048);
            let thread_id = thread::current().id();

            for _ in 0..1024 * call_needed {
                let random = rand::random_range(0..1024 * 512);
                let ptr = unsafe { black_box(malloc(random)) };
                assert!(!ptr.is_null());
                unsafe { black_box(free(ptr)) };
            }

            println!(
                "Async Stress Test, ID: {:?}, Loop: {}, completed no failure",
                thread_id,
                1024 * call_needed
            );
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
}
