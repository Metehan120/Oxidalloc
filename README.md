# Current branch status:
- Optimized paths
- Added hardening without slowing down the allocator
- Improved memory usage
- Removed self-healing path
- Segments for Bitmap
- RADIX Tree
- Many O(1) Optimizations
- Nightly migration | For faster paths
- Likely / Unlikely optimizations
- Fast Big Allocation path
- Kernel Edge Case handling
- More robust segmentation handling
- Many preps before Alpha release

# Current VA Handling:
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

# Found bugs:
- Data race or Corruption in Global / Fixed

# Allocation speed of this branch:
- Free + Malloc <= 7/8ns

# Known Incompatibilities:
- rust-analyzer abort or panic or sigsev after a while / should be fixed after edge case handling
