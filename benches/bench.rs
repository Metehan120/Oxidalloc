use std::{
    hint::black_box,
    sync::{Arc, Barrier},
};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

unsafe extern "C" {
    fn malloc(size: libc::size_t) -> *mut libc::c_void;
    fn free(ptr: *mut libc::c_void);
}

fn shuffle_indices(indices: &mut [usize]) {
    for i in (1..indices.len()).rev() {
        let j = rand::random_range(0..=i);
        indices.swap(i, j);
    }
}

fn bench_alloc_free(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_free");

    // 64B
    group.bench_function("64B", |b| {
        b.iter(|| unsafe {
            let ptr = black_box(malloc(64));
            (ptr as *mut u8).add(1).write(1);
            black_box(free(ptr));
        });
    });

    // 4KB
    group.bench_function("4KB", |b| {
        b.iter(|| unsafe {
            let ptr = black_box(malloc(4096));
            (ptr as *mut u8).add(1).write(1);
            black_box(free(ptr));
        });
    });

    // 1MB
    group.bench_function("1MB", |b| {
        b.iter(|| unsafe {
            let ptr = black_box(malloc(1024 * 1024));
            (ptr as *mut u8).add(1).write(1);
            black_box(free(ptr));
        });
    });

    group.finish();
}

fn bench_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");
    for size in [
        64,
        256,
        512,
        4096,
        16384,
        1024 * 32,
        1024 * 1024 * 2,
        1024 * 1024 * 32,
        1024 * 1024 * 1024,
    ] {
        for num_threads in [1, 2, 4, 8, 12, 16] {
            group.bench_with_input(
                BenchmarkId::from_parameter(format!("{}threads, size{}", num_threads, size)),
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
                                        let p = black_box(malloc(size));
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
    }

    group.finish();
}

fn bench_small_slab(c: &mut Criterion) {
    let mut group = c.benchmark_group("small_slab");
    let sizes = [
        16usize, 32, 48, 64, 80, 96, 128, 160, 192, 256, 320, 384, 512,
    ];
    let batch_sizes = [32usize, 128, 512];

    for &batch in &batch_sizes {
        for &size in &sizes {
            group.bench_with_input(
                BenchmarkId::new(format!("batch{}", batch), format!("{}B", size)),
                &size,
                |b, &size| {
                    let mut ptrs = vec![std::ptr::null_mut(); batch];
                    b.iter_custom(|iters| {
                        let start = std::time::Instant::now();
                        for _ in 0..iters {
                            unsafe {
                                for p in &mut ptrs {
                                    let ptr = black_box(malloc(size as libc::size_t));
                                    (ptr as *mut u8).write(0xA5);
                                    *p = ptr;
                                }
                                for p in &mut ptrs {
                                    black_box(free(*p));
                                }
                            }
                        }
                        start.elapsed()
                    });
                },
            );
        }
    }

    group.finish();
}

fn bench_size_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("size_sweep");
    let sizes = [
        16usize, 32, 48, 64, 80, 96, 128, 160, 192, 256, 320, 384, 512, 768, 1024, 1536, 2048,
        3072, 4096, 8192, 16384, 32768,
    ];

    for &size in &sizes {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}B", size)),
            &size,
            |b, &size| {
                b.iter(|| unsafe {
                    let ptr = black_box(malloc(size as libc::size_t));
                    (ptr as *mut u8).write(0xA5);
                    black_box(free(ptr));
                });
            },
        );
    }

    group.finish();
}

fn bench_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("patterns");
    let sizes = [32usize, 64, 128, 256, 512, 1024];
    let batch = 512usize;
    let mut ptrs = vec![std::ptr::null_mut(); batch];
    let mut indices: Vec<usize> = (0..batch).collect();

    for &size in &sizes {
        group.bench_with_input(
            BenchmarkId::new("lifo", format!("{}B", size)),
            &size,
            |b, &size| {
                b.iter(|| unsafe {
                    for p in &mut ptrs {
                        let ptr = black_box(malloc(size as libc::size_t));
                        (ptr as *mut u8).write(0x5A);
                        *p = ptr;
                    }
                    for p in ptrs.iter().rev() {
                        black_box(free(*p));
                    }
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fifo", format!("{}B", size)),
            &size,
            |b, &size| {
                b.iter(|| unsafe {
                    for p in &mut ptrs {
                        let ptr = black_box(malloc(size as libc::size_t));
                        (ptr as *mut u8).write(0x5A);
                        *p = ptr;
                    }
                    for p in &ptrs {
                        black_box(free(*p));
                    }
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("random_free", format!("{}B", size)),
            &size,
            |b, &size| {
                b.iter(|| unsafe {
                    for p in &mut ptrs {
                        let ptr = black_box(malloc(size as libc::size_t));
                        (ptr as *mut u8).write(0x5A);
                        *p = ptr;
                    }
                    shuffle_indices(&mut indices);
                    for &idx in &indices {
                        black_box(free(ptrs[idx]));
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_fragmentation(c: &mut Criterion) {
    let mut group = c.benchmark_group("fragmentation");
    let sizes = [24usize, 40, 72, 120, 200, 360, 600, 1024];
    let batch = 1024usize;
    let mut ptrs = vec![std::ptr::null_mut(); batch];

    group.bench_function("mixed_half_free_refill", |b| {
        b.iter(|| unsafe {
            for (i, p) in ptrs.iter_mut().enumerate() {
                let size = sizes[i % sizes.len()];
                let ptr = black_box(malloc(size as libc::size_t));
                (ptr as *mut u8).write(0x3C);
                *p = ptr;
            }

            for (i, p) in ptrs.iter().enumerate() {
                if (i & 1) == 0 {
                    black_box(free(*p));
                }
            }

            for (i, p) in ptrs.iter_mut().enumerate() {
                if (i & 1) == 0 {
                    let size = sizes[(i + 3) % sizes.len()];
                    let ptr = black_box(malloc(size as libc::size_t));
                    (ptr as *mut u8).write(0xC3);
                    *p = ptr;
                }
            }

            for p in &ptrs {
                black_box(free(*p));
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_alloc_free,
    bench_contention,
    bench_small_slab,
    bench_size_sweep,
    bench_patterns,
    bench_fragmentation,
);

criterion_main!(benches);
