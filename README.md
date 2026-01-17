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
- Many preps before Alpha release

# Found bugs:
- Data race or Corruption in Global / Fixed

# Allocation speed of this branch:
- Free + Malloc <= 7/8ns
