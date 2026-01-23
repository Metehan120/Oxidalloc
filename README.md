# Oxidalloc

A pure Rust general-purpose memory allocator designed to be used as a malloc replacement via `LD_PRELOAD`.

## Alpha Milestone Update
### Core development is nearly complete. Oxidalloc now features a stable LD_PRELOAD backend and optimized Linux system abstractions.  Active development on final Alpha features is currently occurring in the new-features branch.

## Big Announcement: The first alpha release is coming soon — in the next few weeks (Around February 2026).

## **Huge Update**: Oxidalloc now comes with many contention fixes, lock-free paths, improved performance and more.
## This update includes:
- Improved performance, shaved ~3-4ns off
- Improved contention handling: shouldnt slow down even with 16 threads
- Improved paths: lock-free global with "tagged" pointers which should prevent ABA while being fast (Hopefully, haha.), and lock-free TLS (for ptrim thread)
- Many RSS fixes and better internal fragmentation handling: >%40 RSS drops on most conditions
- LUT for size class searchs: Optimized lookup table for size class searchs, reducing search time by ~50%
- Pushing small block to Global after life time exceeded
- Some optimizations for better cache utilization
- More configuration options
- Better trimming policy
- Better realloc path
## While this update is still under development, please report any issues or feedbacks.

### Note: Oxidalloc still in development and experimental.

## Overview

Oxidalloc is a high-performance allocator written entirely in Rust. It is designed to be ABI-compatible with glibc's malloc family and verified to run system-wide across a full Linux desktop environment.

## ***Important***: This allocator have a experimental mode called "Self Healing", which is comes disabled by default, but you can enable it by setting the environment variable `OX_ENABLE_EXPERIMENTAL_HEALING=1`, *WARNING*: This mode may cause unstability or even security risk.

## Features

* Pure Rust implementation
* Works under `LD_PRELOAD`
* Thread-local fast paths
* Cross-thread frees supported
* Optional debug consistency checks
* Fast: ~60 cycles malloc+free on modern CPUs (10ns on 4.65ghz)

# Compatibility
* Almost completely compatible with Pop!_OS-24.04 (completly compatible with COSMIC Desktop Environment)
* Almost completely compatible with KDE on CachyOS \ Boots and logins in Without any on CachyOS \ *Probably* also compatible with KDE on Fedora
* Compatible with Chromium on most tested OS
* Compatible with Firefox on Pop!_OS-24.04/CachyOS (Tested OSes) \ Means should work fine with Ubuntu and Arch
* Compatible with Python, ROCm, Torch, most runtimes
* Compatbile with tools like Zapret
* Compatible with Pipewire
* Compatible with newer Kernels
* Compatible with Proton/Wine on most cases \ Test still needed but should work fine
* Compatible with Blender as of now

## Tests Needed:
* Fedora
* Gnome / Should work fine but tests needed either way
* Arch
* CachyOS on some Parts
* Proton/Wine

## Incompatibilities
* WARNING: Design only working on 64-BIT systems, incompatible with 32-BIT.
* Due to kernel issues incompatible with some parts of CachyOS
* Under some certain conditions, it may cause fragmentation especially on small allocation storm Apps.

## Benchmarks:

| Function | Speed (ns) |
|-----------|--------------|
| malloc (thread-local path)   |  10            |
| free   (thread-local path)   |  10            |

## Usage

### Build

```bash
cargo build --release
```

### Session-only preload

```bash
export LD_PRELOAD=/path/to/liboxidalloc.so
```

## Known Issues

* Rust-analyzer may cause high memory usage.
* May crash some APPs
* May crash after a while during AI workloads.

## License

Licensed under [MIT](LICENSE).

## Status

Actively developed.

## Contributing

Contributions are welcome! Please read our [contributing guidelines](CONTRIBUTING.md).

## Acknowledgments

* Special thanks to the developers of the [Rust](https://www.rust-lang.org/) programming language.

## Current code documentation

* There will be documentation during the rewrite process.

**Note**: This allocator is experimental. Test thoroughly before production use. Benchmark your specific workload.
