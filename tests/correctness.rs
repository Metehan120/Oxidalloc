use std::{
    collections::BTreeSet,
    env,
    hint::black_box,
    os::raw::c_void,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

unsafe extern "C" {
    pub fn malloc(size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
    pub fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
}

#[derive(Debug, PartialEq, Eq)]
pub struct SafeWrapper(*mut c_void);
unsafe impl Send for SafeWrapper {}
unsafe impl Sync for SafeWrapper {}

#[derive(Debug, PartialEq)]
pub struct SafePtr(Vec<SafeWrapper>);
unsafe impl Send for SafePtr {}
unsafe impl Sync for SafePtr {}

impl SafePtr {
    fn push(&mut self, ptr: *mut c_void) {
        self.0.push(SafeWrapper(ptr));
    }
}

fn test_inner(loops: usize, op: fn() -> *mut c_void) -> usize {
    let num_thread = thread::available_parallelism().unwrap();
    let worker_count = Arc::new(Mutex::new(num_thread.get()));
    let global_allocated: Arc<Mutex<Vec<SafePtr>>> = Arc::new(Mutex::new(Vec::new()));
    let mut actual = Vec::new();

    for _ in 0..num_thread.get() {
        let global_allocated = Arc::clone(&global_allocated);
        let worker_count = Arc::clone(&worker_count);

        thread::spawn(move || {
            let mut allocated = SafePtr(Vec::new());

            for _ in 0..loops {
                let ptr = black_box(op());
                allocated.push(ptr);
            }

            {
                let mut global = global_allocated.lock().unwrap();
                global.push(allocated);
            }

            {
                let mut worker = worker_count.lock().unwrap();
                *worker -= 1;
            }
        });
    }

    loop {
        {
            let worker = worker_count.lock().unwrap();
            if *worker == 0 {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    for block in global_allocated.lock().unwrap().iter() {
        for i in block.0.iter() {
            actual.push(i.0);
        }
    }

    let mut seen = BTreeSet::new();
    for &ptr in &actual {
        if ptr.is_null() {
            break;
        }
        if !seen.insert(ptr) {
            panic!("Duplicate pointer");
        }
        unsafe { free(ptr) };
    }

    num_thread.get()
}

#[test]
fn test_global_and_tls_correctness_under_multithread() {
    eprintln!(
        "This test (test_global_and_tls_correctness_under_multithread) tests absolute worst scenerio for allocator does not mirror real performance"
    );
    let loop_needed = env::var("OX_TEST_LOOP")
        .unwrap_or("10000".to_string())
        .parse()
        .unwrap_or(10_000);
    unsafe {
        test_inner(loop_needed, || malloc(16));
    }
}

#[test]
fn big_allocation_multithread_correctness() {
    eprintln!(
        "This test (big_allocation_multithread_correctness) tests absolute worst scenerio for allocator does not mirror real performance"
    );
    eprintln!(
        "Expect slow performance for this test, Oxidalloc hasn't optimized for scenario test yet"
    );

    // For example: 100 * 12 = 1200, 1200 * 32 = 38400mb = 38.4GB allocation
    let loop_needed = env::var("OX_TEST_LOOP")
        .unwrap_or("100".to_string())
        .parse()
        .unwrap_or(100)
        .max(100);

    unsafe {
        let start = Instant::now();
        let thread_count = test_inner(loop_needed, || malloc(1024 * 1024 * 32));
        let end = start.elapsed().as_nanos() as f64;
        eprintln!(
            "Per thread performance (big_allocation_multithread_correctness), including mutex etc.: {}",
            (end / thread_count as f64) / loop_needed as f64
        );
    };
}

#[test]
fn realloc_growth_multithread_correctness() {
    eprintln!(
        "This test (realloc_growth_multithread_correctness) tests absolute worst scenerio for allocator does not mirror real performance"
    );
    let loop_needed = env::var("OX_TEST_LOOP")
        .unwrap_or("10000".to_string())
        .parse()
        .unwrap_or(10_000);

    unsafe {
        let start = Instant::now();
        let thread_count = test_inner(loop_needed, || {
            let ptr = malloc(1024 * 8);
            realloc(ptr, 1024 * 16)
        });
        let end = start.elapsed().as_nanos() as f64;
        eprintln!(
            "Per thread performance (realloc_growth_multithread_correctness), including mutex etc.: {}",
            (end / thread_count as f64) / loop_needed as f64
        );
    };
}

#[test]
fn realloc_shrink_multithread_correctness() {
    eprintln!(
        "This test (realloc_shrink_multithread_correctness) tests absolute worst scenerio for allocator does not mirror real performance"
    );
    let loop_needed = env::var("OX_TEST_LOOP")
        .unwrap_or("10000".to_string())
        .parse()
        .unwrap_or(10_000);

    unsafe {
        let start = Instant::now();
        let thread_count = test_inner(loop_needed, || {
            let ptr = malloc(1024 * 16);
            realloc(ptr, 1024 * 8)
        });
        let end = start.elapsed().as_nanos() as f64;
        eprintln!(
            "Per thread performance (realloc_shrink_multithread_correctness), including mutex etc.: {}",
            (end / thread_count as f64) / loop_needed as f64
        );
    };
}

#[test]
fn malloc_relloc_random_stress_test() {
    eprintln!(
        "This test (malloc_relloc_random_stress_test) tests absolute worst scenerio for allocator does not mirror real performance"
    );
    let loop_needed = env::var("OX_TEST_LOOP")
        .unwrap_or("1000".to_string())
        .parse()
        .unwrap_or(1000)
        / 100;

    for _ in 0..100 {
        unsafe {
            let start = Instant::now();
            let thread_count = test_inner(loop_needed, || {
                let random_malloc = rand::random_range(0..1024 * 1024 * 2);
                let random_realloc = rand::random_range(0..1024 * 1024 * 2);
                let ptr = malloc(random_malloc);
                realloc(ptr, random_realloc)
            });
            let end = start.elapsed().as_nanos() as f64;
            eprintln!(
                "Per thread performance (malloc_relloc_random_stress_test), including mutex etc.: {}",
                (end / thread_count as f64) / loop_needed as f64
            );
        }
    }
}
