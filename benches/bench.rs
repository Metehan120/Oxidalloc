use std::{
    hint::black_box,
    sync::{Arc, Barrier},
};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

unsafe extern "C" {
    fn malloc(size: libc::size_t) -> *mut libc::c_void;
    fn free(ptr: *mut libc::c_void);
}

fn bench_alloc_free(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_free");

    // 64B
    group.bench_function("64B", |b| {
        b.iter(|| unsafe {
            let ptr = black_box(malloc(64));
            black_box(free(ptr));
        });
    });

    // 4KB
    group.bench_function("4KB", |b| {
        b.iter(|| unsafe {
            let ptr = black_box(malloc(4096));
            black_box(free(ptr));
        });
    });

    // 1MB
    group.bench_function("1MB", |b| {
        b.iter(|| unsafe {
            let ptr = black_box(malloc(1024 * 1024));
            black_box(free(ptr));
        });
    });

    group.finish();
}

fn bench_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");

    for num_threads in [1, 2, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}threads", num_threads)),
            &num_threads,
            |b, &num_threads| {
                b.iter_custom(|iters| {
                    let barrier = Arc::new(Barrier::new(num_threads));

                    let handles: Vec<_> = (0..num_threads)
                        .map(|_| {
                            let barrier = barrier.clone();
                            std::thread::spawn(move || unsafe {
                                barrier.wait();
                                for _ in 0..(iters / num_threads as u64) {
                                    let p = black_box(malloc(64));
                                    black_box(free(p));
                                }
                            })
                        })
                        .collect();

                    let start = std::time::Instant::now();

                    for h in handles {
                        h.join().unwrap();
                    }

                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

fn bench_size_class_lookup(c: &mut Criterion) {
    let sizes = [16, 64, 256, 1024, 4096, 65536, 1048576];

    c.bench_function("size_class_lookup", |b| {
        b.iter(|| {
            for &size in &sizes {
                black_box(oxidalloc::slab::match_size_class(black_box(size)));
            }
        });
    });
}

criterion_group!(
    benches,
    bench_alloc_free,
    bench_contention,
    bench_size_class_lookup
);
criterion_main!(benches);
