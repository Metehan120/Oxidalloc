use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::ptr::null_mut;
use std::sync::{Arc, Barrier};
use std::thread;

use oxidalloc::OxHeader;
use oxidalloc::slab::interconnect::ICC;
use oxidalloc::slab::thread_local::ThreadLocalEngine;

unsafe fn create_dummy_header() -> *mut OxHeader {
    let header = Box::into_raw(Box::new(OxHeader {
        next: null_mut(),
        class: 0,
        magic: 0,
        life_time: 0,
    }));
    header
}

fn bench_tls(c: &mut Criterion) {
    let mut group = c.benchmark_group("tls_speed");

    group.bench_function("get_or_init", |b| {
        b.iter(|| unsafe {
            black_box(ThreadLocalEngine::get_or_init());
        });
    });

    group.bench_function("push_pop", |b| unsafe {
        let tls = ThreadLocalEngine::get_or_init();
        let header = create_dummy_header();

        b.iter(|| {
            tls.push_to_thread(0, black_box(header));
            let popped = tls.pop_from_thread(0);
            black_box(popped);
        });

        let _ = Box::from_raw(header);
    });

    group.finish();
}

unsafe fn get_icc() -> *mut oxidalloc::slab::interconnect::InterConnectCache {
    std::ptr::addr_of_mut!(ICC)
}

fn bench_icc(c: &mut Criterion) {
    let mut group = c.benchmark_group("icc_speed");

    group.bench_function("push_pop_local", |b| {
        black_box(unsafe {
            let icc = get_icc();
            (*icc).ensure_cache();
            let header = create_dummy_header();

            b.iter(|| {
                (*icc).try_push(0, black_box(header), black_box(header), 1, false, false);
                let popped = (*icc).try_pop(0, 1, false);
                black_box(popped);
            });

            let _ = Box::from_raw(header);
        })
    });

    group.bench_function("steal", |b| {
        b.iter_custom(|iters| {
            let barrier_inner = Arc::new(Barrier::new(2));
            let bi_c = barrier_inner.clone();

            let handle = thread::spawn(move || {
                black_box(unsafe {
                    let icc = get_icc();
                    let header = create_dummy_header();
                    (*icc).ensure_cache();
                    for _ in 0..iters {
                        (*icc).try_push(0, black_box(header), black_box(header), 1, false, false);
                        bi_c.wait();
                    }

                    let _ = Box::from_raw(header);
                })
            });

            unsafe {
                let icc = get_icc();
                (*icc).ensure_cache();
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    barrier_inner.wait();
                    let popped = (*icc).try_pop(0, 1, false);
                    black_box(popped);
                    if !popped.is_null() {}
                }
                let elapsed = start.elapsed();

                handle.join().unwrap();

                elapsed
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_tls, bench_icc);
criterion_main!(benches);
