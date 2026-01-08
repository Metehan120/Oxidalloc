unsafe extern "C" {
    fn malloc(size: libc::size_t) -> *mut libc::c_void;
    fn free(ptr: *mut libc::c_void);
    fn malloc_usable_size(ptr: *mut libc::c_void) -> libc::size_t;
}

#[test]
fn measure_slab_fragmentation() {
    unsafe {
        println!("\nSlab Internal Fragmentation");

        let mut ptrs = Vec::new();

        for _ in 0..10_000 {
            let ptr = malloc(4000);
            if !ptr.is_null() {
                ptrs.push(ptr);
            }
        }

        let requested = 4000 * ptrs.len();
        let mut actual = 0;

        for &ptr in &ptrs {
            actual += malloc_usable_size(ptr);
        }

        let wasted = actual - requested;
        let fragmentation = (wasted as f64 / actual as f64) * 100.0;

        println!("Requested: {} MB", requested / (1024 * 1024));
        println!("Actually used: {} MB", actual / (1024 * 1024));
        println!("Wasted (internal frag): {} MB", wasted / (1024 * 1024));
        println!("Internal fragmentation: {:.2}%", fragmentation);

        for (i, ptr) in ptrs.iter().enumerate() {
            if i % 3 == 0 {
                free(*ptr);
            }
        }

        let mut reused = 0;
        for _ in 0..3333 {
            let ptr = malloc(4000);
            if !ptr.is_null() {
                reused += 1;
                free(ptr);
            }
        }

        println!("Freed 3333 blocks, reused {} blocks", reused);
        println!(
            "Freelist efficiency: {:.2}%",
            (reused as f64 / 3333.0) * 100.0
        );

        for (i, ptr) in ptrs.iter().enumerate() {
            if i % 3 != 0 {
                free(*ptr);
            }
        }
    }
}

#[test]
fn measure_thread_cache_fragmentation() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    println!("\nThread Cache Fragmentation");

    let allocated = Arc::new(AtomicUsize::new(0));
    let freed = Arc::new(AtomicUsize::new(0));

    let a1 = allocated.clone();
    let t1 = thread::spawn(move || unsafe {
        for _ in 0..10_000 {
            let ptr = malloc(128);
            if !ptr.is_null() {
                a1.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    t1.join().unwrap();

    println!(
        "Thread 1 allocated {} blocks (then exited)",
        allocated.load(Ordering::Relaxed)
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    let f2 = freed.clone();
    let t2 = thread::spawn(move || unsafe {
        let mut local_ptrs = Vec::new();

        for _ in 0..10_000 {
            let ptr = malloc(128);
            if !ptr.is_null() {
                local_ptrs.push(ptr);
            }
        }

        for ptr in local_ptrs {
            free(ptr);
            f2.fetch_add(1, Ordering::Relaxed);
        }
    });

    t2.join().unwrap();

    println!(
        "Thread 2 allocated and freed {} blocks",
        freed.load(Ordering::Relaxed)
    );
    println!("Cross-thread reuse: Should have reused from global pool");
}
