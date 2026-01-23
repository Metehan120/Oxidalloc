# Current branch status:
- NUMA-awareness
- Added hardening (hardened-linked-list + hardened-malloc) — expect slowdowns on real workloads; stress-ng shows ~8–9× slowdown when hardened-linked-list is enabled.
- TLS usage caps (Max 128KB per class) - No more memory growing non-stop
- Randomized Bitmap allocation: Used SplitMix64 style randomization to avoid predictable patterns.
- Lazy block initialization for better RSS (Extreme drops of memory usage on some workloads)
- Removed self-healing path
- Segments for Bitmap
- RADIX Tree
- Many O(1) Optimizations
- Nightly migration | For faster paths
- Likely / Unlikely optimizations
- Fast Big Allocation path
- Kernel Edge Case handling
- More robust segmentation handling
- Removed PTRIM, no more need after TLS caps
- Added inplace realloc growth
- Official stress test suite (from hell)
- New Build flags: 
  * tls-model=initial-exec
  * link-arg=-Wl,-z,now
  - Expect 1.5-2x better performance
- Internal syscall abstractions for future compatibility works
- Many preps before Alpha release

# Optimizations:
- **Optimized realloc path / now extremely faster**
- **Optimized Bitmap**
- **Optimized free paths**
- Optimized malloc paths
- Optimized aligned block (posix-memalign) handling
- Optimized TLS
- Optimized Global
- Improved memory usage based on TLS caps and segment handling

  
# Current VA Handling:
# Verified: VA management and Radix Tree bookkeeping stress-tested up to 13TB.
- Able to handle ~300–500 GB of virtual address space under pathological worst-case conditions without crashing or corrupting state.
### Worst-case scenario tested:
- VA ranges are never touched (no page faults).
  * Mappings use MAP_NORESERVE, so the kernel may legally:
  * reuse identical VA ranges,
  * or heavily fragment the address space.
- No assumptions are made about monotonic or unique VA returns.
- Even under these conditions:
  * The allocator detects collisions correctly.
  * Failed reservations are handled cleanly.
  * Segment growth backs off and adapts.
  * No overlapping mappings, no hangs, no undefined behavior.
- Under real workloads (pages touched / reused):
  * VA reuse stabilizes.
  * Kernel fragmentation pressure drops significantly.
  * Effective VA capacity is higher and more stable than worst-case tests.

# Allocation speed of this branch:
- Free + Malloc <= 5/6ns
