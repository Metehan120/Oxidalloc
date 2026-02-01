# Oxidalloc Architecture

This document describes the high-level design and data flow of Oxidalloc. It is meant to give
contributors enough context to reason about correctness and performance without digging through
every file.

## Goals (in practice)
- Very fast hot-path malloc/free for small allocations.
- Stable long-running behavior (bounded TLS growth, trimming, RSS control).
- Strong ASLR and hardened modes (optional) without giving up core throughput.
- Safe fork handling (reinitialize state that cannot be safely shared).

## Layout and key modules
- `src/abi/`: C ABI surface (`malloc`, `free`, `realloc`, `calloc`, `posix_memalign`, etc.).
- `src/slab/`: Size-classed allocator, thread-local caches, global/interconnect cache.
- `src/va/`: Virtual address reservation and VA bitmap management.
- `src/big_allocation.rs`: Page-granular allocations for large sizes.
- `src/trim/`: Trimming policies and background trim thread.
- `src/sys/`: Raw syscalls and platform abstractions.
- `src/internals/`: Locks, once, hashmaps, env helpers.

## Core data structures
- **OxHeader** (`src/lib.rs`): placed immediately before each payload. Stores class, magic, and
  a linked-list `next` pointer.
- **Size classes** (`src/slab/mod.rs`): fixed sizes up to 2 MiB. Above that uses big allocation.
- **VA bitmap / segments** (`src/va/bitmap.rs`): tracks reserved ranges and avoids collisions.
- **InterConnect Cache (ICC)** (`src/slab/interconnect.rs`): per-CPU sharded cache used as the
  global exchange point. This implementation uses `sched_getcpu()` (not RSEQ).

## Allocation paths
### Small/medium allocations
1. `malloc` chooses a size class (fast LUT for <= 4096 bytes, otherwise `match_size_class`).
2. Thread-local cache is used first (`ThreadLocalEngine`).
3. On miss, `try_fill` pulls a batch from ICC (global exchange).
4. If ICC is empty, `bulk_fill` allocates a fresh slab segment.

### Big allocations (> 2 MiB)
- `big_malloc` reserves VA via `VA_MAP`, then commits pages with `mmap/mprotect`.
- Metadata goes into `BIG_ALLOC_MAP`, and the header class is set to `100`.

### Aligned allocations
- `posix_memalign` over-allocates and stores a tag + original pointer in front of the aligned
  return address. `malloc_usable_size`, `free`, and `realloc` detect the tag and walk back to
  the true header.

### Realloc
- Fast paths for same-class and in-place growth/shrink (uses VA bitmap).
- For large class changes, fall back to allocate-copy-free.

## Free path
1. Validate header magic (hardened-malloc adds extra checks).
2. If `class == 100`, free via `big_free`.
3. Otherwise push into thread-local cache.
4. If TLS cache is full, push to ICC in batches.

## Virtual address management
- VA is reserved in large chunks (bitmap segments). This allows predictable address-space
  accounting and reuse detection.
- `VA_MAP` can return ranges out of order; overlap is detected via segment metadata.
- `OX_MAX_RESERVATION` controls the maximum reservation size (power-of-two, clamped).
- The allocator caps request size at ~3 GiB due to minimum bitmap chunk sizing.

## InterConnect Cache (ICC)
- Per-CPU shards of lock-free lists (one list per size class).
- `try_push` batches freed blocks into the shard for the calling CPU.
- `try_pop` tries the local shard, then steals from other CPUs.
- Hardened-linked-list mode XOR-masks pointers with `NUMA_KEY`.
- `get_cpu_count()` determines shard count at initialization.

## Trimming and memory pressure
- A background trim thread periodically updates `OX_CURRENT_STAMP` and triggers global trimming.
- `GTrim.trim` walks ICC usage and reclaims unused blocks.
- Memory pressure is estimated from `sysinfo` (with `mem_unit` applied).

## Fork handling
- Fork handlers are registered during `boot_strap`.
- On fork, locks and once guards are reset, TLS state is reinitialized, and fallback allocators
  are re-bound.
- In hardened-linked-list mode, ICC locks are reset if initialized.

## Configuration (environment)
- `OX_USE_THP`: enable THP (`madvise(HUGEPAGE)` on eligible allocations).
- `OX_TRIM_THRESHOLD`: trim threshold (clamped to >= 1 MiB).
- `OX_MAX_RESERVATION`: VA reservation cap (clamped to [16 GiB, 256 TiB], power-of-two).

## Safety / hardening modes
- `hardened-malloc`: validates magic values on alloc/free.
- `hardened-linked-list`: XOR-masks next pointers and uses stronger global locks.
- These modes trade throughput for integrity and exploit resistance.

## Testing notes
- Stress tests are under `tests/` and `benches/`.
- The global/interconnect contention test is in `src/slab/global.rs` (test module).
