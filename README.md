# Oxidalloc Alpha-2

Oxidalloc is a general-purpose Rust allocator for Linux that prioritizes predictable,
low-latency allocation, long-running stability, and modern hardening options. It is not a
thin wrapper over the system allocator: it owns the full allocation stack end-to-end,
including size classes, caches, virtual address (VA) reservations, and trimming policy.

In practice, Oxidalloc is designed to behave like a high-performance engine:
fast hot paths for small allocations, robust handling of large allocations, and
explicit control over address-space and memory reclamation.

## What Oxidalloc is (and is not)

- A full allocator implementation with its own data structures, not a shim.
- Focused on Linux: uses raw syscalls and Linux-specific primitives.
- Optimized for low-latency paths under contention (ICC + TLS caches).
- General-purpose allocator intended for real-world applications.
- Built with security modes in mind (optional hardening).
- Not a drop-in replacement for every libc edge case, though compatibility is close to full in practice.

## **Warning:** Alpha quality

> [!CAUTION]
> Oxidalloc is in early alpha. It is **not production-ready**. You should expect crashes, memory corruption, or deadlocks when running complex or highly-contended workloads.

- **Stability**: While core paths are verified with `loom` and stress tests, edge cases in `realloc` or heavy cross-thread contention may still trigger `SIGSEGV` or `SIGABRT` even if low probability.
- **Performance**: Small-block hot paths are fast in microbenchmarks, but real-world "cache-unfriendly" workloads may exhibit different characteristics. Tuning is ongoing.
- **Memory Footprint**: Long-running Resident Set Size (RSS) behavior is still being optimized. trimmers may not be aggressive enough for all use cases.
- **Security**: Hardening modes (pointer tagging, XOR lists) provide a layer of protection but have **not been professionally audited**.
- **Platform**: Primarily tested on Linux x86_64. Other Linux architectures may have subtle syscall or memory ordering issues.
- **Unsafe Code**: The codebase relies heavily on manual pointer management and raw syscalls. Use only in environments where you can afford an experimental memory manager.
- **Modern glibc requirement**: Because of migration through `rseq` modern glibc versions are required (2.35+).

## Status

- Public alpha (active development). Expect instability and rough edges.
- Linux only (raw syscalls).
- Nightly Rust required (for likely+unlikely hints and thread-local storage).

## Design highlights

- Per-CPU InterConnect Cache replaces a single global heap and scales with core count.
- **Radix Tree**: VA bitmap + radix tree makes large VA reservations and pointer identification safe even under high concurrency.
- In-place `realloc` growth/shrink when address space allows it.
- TLS cache caps prevent unbounded growth in long-running workloads.
- Hardened modes add pointer integrity checks and XOR-masked lists.
- **Fork-Safe**: Fork-aware reset logic (via `pthread_atfork`) ensures that internal state, locks, and TLS are safely reinitialized across clones.

## Architecture (short version)

High-level flow is described in `ARCHITECTURE.md`. In brief:

Oxidalloc controls the full allocation lifecycle:
it reserves address space, maps pages on demand, tracks ownership via headers, and
reclaims memory via trimming. This is not just a cache on top of `malloc`; it is a
complete allocator with its own VA management, metadata, and cache topology.

Key files to start reading:

- `src/abi/` (C ABI entry points)
- `src/inner/` (Allocator internal components)
- `src/slab/` (size classes, TLS caches, ICC)
- `src/va/` (VA bitmap + reservations)
- `src/trim/` (trimming and background thread)
- `src/big_allocation.rs` (big allocations)

### Allocation path (small/medium)

1. `malloc` maps size to a class (fast LUT for <= 4096 bytes).
2. Thread-local cache is used first.
3. On miss, a batch is pulled from ICC (per-CPU shard).
4. If ICC is empty, `bulk_fill` creates a fresh slab segment.

### Allocation path (big)

- Sizes > 2 MiB go through `big_malloc`, reserve VA via `VA_MAP`, then commit pages with
  `mmap`/`mprotect`. Metadata is tracked in `BIG_ALLOC_MAP`.

### Free path

1. Validate header magic (hardened modes add extra checks).
2. If big (`class == 100`), free via `big_free`.
3. Otherwise push into TLS; if TLS full, batch to ICC.

### Realloc path

- Fast in-place growth/shrink when possible (VA bitmap).
- Otherwise allocate-copy-free.

## InterConnect Cache (ICC)

ICC replaces a single global list with per-CPU shards:

- Pushes and pops are batched for amortized atomic cost.
- Local shard is preferred; other shards are used for victim stealing.

## VA management

- Virtual address reservations are tracked in a bitmap + radix tree.
- Overlaps, reuse, and fragmentation are handled explicitly.
- VA reservation cap is controlled via `OX_MAX_RESERVATION`.
- Base hints are randomized to strengthen ASLR behavior.

## Trimming

- A background trim thread updates a global timestamp and triggers trimming.
- Memory pressure is estimated using `sysinfo`.

## Fork handling

- Fork handlers reset locks, one-time init state, TLS, and fallback hooks.
- Hardened ICC locks are reset on fork if initialized.

## Hardening (optional)

- `hardened-malloc`: validates magic values to detect corruption.
- `hardened-linked-list`: XOR-masks pointers + stronger global locks.
- Expect overhead; not audited yet.

## Proxy Mode and Fallback

When Oxidalloc is used as a `cdylib` (e.g., via `LD_PRELOAD`), it automatically enters **Proxy Mode**. It uses an internal radix tree to identify whether a pointer belongs to its managed regions.
- **Owned Pointers**: Handled by Oxidalloc's high-speed caches.
- **External Pointers**: Safely delegated to the system's fallback allocator (glibc or whatever comes after).
This ensures compatibility with libraries that may have allocated memory before Oxidalloc was loaded or that bypass the standard allocation path.

## Configuration (environment)

- `OX_FORCE_THP=1` — Forces Transparent Huge Pages (THP) for all big allocations by aligning to 2MB and using `madvise(HUGEPAGE)`.
- `OX_TRIM_THRESHOLD=<bytes>` — Minimum threshold of free memory before the trim thread reclaims it (clamped to >= 1 MiB).
- `OX_MAX_RESERVATION=<bytes>` — Total Virtual Address (VA) reservation cap (power-of-two, clamped to [16 GiB, 256 TiB]).
- `OX_DISABLE_TRIM_THREAD=1` — Disables the background trimming thread.
- `OX_DISABLE_THP=1` — Disables transparent huge pages. If `OX_FORCE_THP` is set, this is ignored.

## Limits / tradeoffs

- Allocation size is capped at ~3 GiB due to minimum bitmap chunk sizing.
- The allocator is optimized for low latency; extreme hardening trades throughput for safety.
- RSS behavior is typically within ~10% of other allocators; within tested workloads and limits, or sometimes better,
  tested without the trim thread.

## Build

```bash
cargo +nightly build --release
```

This builds `liboxidalloc.so` in `target/release/`.

## ABI and integration

- Exposes standard C allocator symbols.
- **Complete C ABI Support**:
    - `malloc`, `free`, `realloc`, `calloc`
    - `posix_memalign`, `memalign`, `aligned_alloc`, `valloc`, `pvalloc`
    - `reallocarray`, `recallocarray`
    - `malloc_usable_size`, `malloc_trim`
- Intended to be loaded via `LD_PRELOAD` or linked as a `cdylib`.
- “Just enough” compatibility: optimized behavior over strict libc edge-case parity.

## Usage (LD_PRELOAD)

```bash
LD_PRELOAD=./target/release/liboxidalloc.so <your_program>
```

## Usage as Global Allocator

You can use Oxidalloc as the global allocator in your Rust project. This requires the `global-alloc` feature.

### 1. Update `Cargo.toml`

```toml
[dependencies]
oxidalloc = { version = "1.0.0-public-alpha-2", features = ["global-alloc"] }
```

### 2. Configure in `main.rs` or `lib.rs`

```rust
use oxidalloc::{Oxidalloc, OxidallocConfig};

#[global_allocator]
static ALLOC: Oxidalloc = Oxidalloc::new();

// Or with custom configuration:
/*
static ALLOC: Oxidalloc = Oxidalloc::new_with_config(OxidallocConfig {
    disable_trim: false,
    disable_thp: false,
    force_thp: false,
    ..OxidallocConfig::new()
});
*/

fn main() {
    let mut v = Vec::new();
    v.push(1);
    println!("Allocations are now handled by Oxidalloc!");
}
```

> [!IMPORTANT]
> When using `global-alloc`, ensure you are building with `+nightly` as Oxidalloc relies on nightly features for performance.

## Verified Lock-Free Structures

Oxidalloc's core synchronization primitives, including the InterConnect Cache (ICC), are verified using [loom](https://github.com/tokio-rs/loom) to ensure correctness under concurrency and detect potential data races or ABA issues.

To run the verification tests:
```bash
cargo +nightly test --test "*loom"
```

## Features

- `hardened-malloc`
- `hardened-linked-list` (implies hardened-malloc)
- `debug`

Example:

```bash
cargo +nightly build --release --features hardened-linked-list
```

## Early benchmarks (Ryzen 5 5600X, CachyOS)

> Results vary by workload, kernel, and machine configuration.

These numbers are preliminary and will be expanded as more benchmark suites are run.

- 64B malloc+free (TLS, bench): ~4.8 ns
- 4KB malloc+free (TLS, bench): ~4.8 ns
- 1MB malloc+free (TLS, bench): ~6.5 ns
- stress-ng: ~44M bogo ops/s

### sh6benchN

Total elapsed time: 0.00 (0.1617 CPU)  
Clock ticks read from register: 601,361,296  
Page faults: 13,390

For more extensive benchmark suites, see [`benchmarks`](benchmarks/OVERVIEW.md).

## Tests and benchmarks

Oxidalloc is verified against a comprehensive suite of integration and stress tests:

### Integration Tests
- `tests/correctness.rs`: Multi-threaded allocation/free consistency (e.g., `Arc<Mutex<Vec>>` patterns).
- `tests/global_alloc_verification.rs`: Verifies performance and correctness as a `#[global_allocator]`.
- `tests/abi_verification.rs`: Ensures C ABI parity and cross-allocator pointer handling.
- `tests/basic.rs`: Smoke tests for core allocation logic.

### Concurrency Verification
- `tests/icc_loom.rs`: Uses [loom](https://github.com/tokio-rs/loom) to verify the lock-free InterConnect Cache for data races and ABA issues.
- `tests/va_loom.rs`: Loom-verified virtual address management and claim/release logic.

### Stress and Performance
- `tests/stress.rs`: Brutal multi-threaded stress tests with random allocation patterns.
- `tests/stress_internals.rs`: Low-level bitmap and slab management stress testing.
- `tests/metrics.rs`: Measures fragmentation, freelist efficiency, and cache utilization | Optimistic.

### Running Tests
```bash
# Run all tests
cargo +nightly test --release -- --no-capture

# Run concurrency verification (Loom)
cargo +nightly test --test "*loom"
```

For more extensive benchmark suites, see [`benchmarks`](benchmarks/OVERVIEW.md).

## Contributing

See `CONTRIBUTING.md`.
