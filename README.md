# Oxidalloc

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

- Do not expect rock-solid stability yet.
- Small-block performance is still being tuned in real-world workloads; microbench numbers below are preliminary.
- Long-running RSS behavior can vary; it is still being tuned and tested.
- Hardening modes are not audited; use with caution.
- Extensive use of `unsafe` is required for direct syscalls, pointer arithmetic, and lock-free structures.

## Status

- Public alpha (active development). Expect instability and rough edges.
- Linux only (raw syscalls).
- Nightly Rust required (for likely+unlikely hints and thread-local storage).

## Design highlights

- Per-CPU InterConnect Cache replaces a single global heap and scales with core count.
- VA bitmap + radix tree makes large VA reservations safe and collision-aware.
- In-place `realloc` growth/shrink when address space allows it.
- TLS cache caps prevent unbounded growth in long-running workloads.
- Hardened modes add pointer integrity checks and XOR-masked lists.
- Fork-aware reset logic to avoid sharing unsafe state across fork.

## Architecture (short version)

High-level flow is described in `ARCHITECTURE.md`. In brief:

Oxidalloc controls the full allocation lifecycle:
it reserves address space, maps pages on demand, tracks ownership via headers, and
reclaims memory via trimming. This is not just a cache on top of `malloc`; it is a
complete allocator with its own VA management, metadata, and cache topology.

Key files to start reading:

- `src/abi/` (C ABI entry points)
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
- Uses `sched_getcpu()` for compatibility (not RSEQ; some custom kernels don't support RSEQ).

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

## Configuration (environment)

- `OX_FORCE_THP=1` — forcing THP (`madvise(HUGEPAGE)` for every big allocations by aligning to 2MB)
- `OX_TRIM_THRESHOLD=<bytes>` — minimum trim threshold (clamped to >= 1 MiB)
- `OX_MAX_RESERVATION=<bytes>` — VA reservation cap (power-of-two, clamped to [16 GiB, 256 TiB])

## Limits / tradeoffs

- Allocation size is capped at ~3 GiB due to minimum bitmap chunk sizing. (Exceeding this cap returns NULL and sets ENOMEM.)
- The allocator is optimized for low latency; extreme hardening trades throughput for safety.
- RSS behavior is typically within ~10% of other allocators; within tested workloads and limits, or sometimes better,
  tested without the trim thread.

## Build

```bash
cargo +nightly build --release
```

This builds `liboxidalloc.so` in `target/release/`.

## ABI and integration

- Exposes standard C allocator symbols (`malloc`, `free`, `realloc`, `calloc`, `posix_memalign`, etc.).
- Intended to be loaded via `LD_PRELOAD` or linked as a `cdylib`.
- “Just enough” compatibility: optimized behavior over strict libc edge-case parity.

## Usage (LD_PRELOAD)

```bash
LD_PRELOAD=./target/release/liboxidalloc.so <your_program>
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

- Tests live in `tests/`.
- Criterion benchmarks in `benches/`.
- Stress tests are included and meant to be brutal.

## Contributing

See `CONTRIBUTING.md`.
