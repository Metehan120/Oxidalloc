# Oxidalloc

A pure Rust general-purpose memory allocator designed to be used as a malloc replacement via `LD_PRELOAD`.

## Overview

Oxidalloc is a high-performance allocator written entirely in Rust. It is designed to be ABI-compatible with glibc's malloc family and verified to run system-wide across a full Linux desktop environment.

## Tested on Fedora
## IMPORTANT: New update may add More Compatibility but also May Break some of them, please report any issues you encounter.

## Features

* Pure Rust implementation
* Works under `LD_PRELOAD`
* System-wide compatible (systemd, KDE Plasma, GTK apps, browsers, Wine/Proton, etc.)
* Thread-local fast paths
* Cross-thread frees supported
* Optional debug consistency checks
* Fast: ~120 cycles malloc+free on modern CPUs (20ns on 4.65ghz) | Haven't started optimizing yet

## Incompatibilities
* WARNING: Design only working on 64-BIT systems, incompatible with 32-BIT.
* Incompatible with Fedora Firefox for now, use Flatpak Firefox if possible (if you are on another distro, use the official Firefox package). This appears to be due to Fedora-specific patches/build configuration in their Firefox package that conflicts with LD_PRELOAD allocators.
* Under some certain conditions, it may cause fragmentation.

## Architecture

* `malloc.rs` – core allocation logic
* `free.rs` – deallocation path and metadata validation
* `realloc.rs` – resizing logic
* `calloc.rs` – zero-initialized allocation
* `align.rs` – alignment-specific allocators
* `thread_local.rs` – per-thread caches
* `global.rs` – global allocator paths and fallbacks
* `internals.rs` – metadata, size classes, constants, va reservation
* `trim.rs` – memory trimming and page returns
* `lib.rs` – exported C ABI symbols

## Benchmarks:

| Function | Speed (ns) |
|-----------|--------------|
| malloc (thread-local path)   |  20            |
| free   (thread-local path)   |  20 + 10 (trim)           |

## Usage

### Build

```bash
cargo build --release
```

### System-wide preload **(example, do not attempt may cause system instability, but mostly works fine)**

```bash
echo "/path/to/liboxidalloc.so" | sudo tee /etc/ld.so.preload
```

### Session-only preload

```bash
export LD_PRELOAD=/path/to/liboxidalloc.so
```

## Known Issues

* When Firefox (Only Fedora package) browsers starting, UI not loading correctly
* High memory usage when using Rust Analyzer. | Working on it.
* May crash some APPS
* May crash after a while during AI workloads, because of VA exhaustion but after current update 'VA_MAP' mostly fixed it.

## License

Licensed under [MIT](LICENSE).

## Status

Actively developed.

## Contributing

Contributions are welcome! Please read our [contributing guidelines](CONTRIBUTING.md).

## Acknowledgments

* Special thanks to the developers of the [Rust](https://www.rust-lang.org/) programming language.


## Current code documentation

* There's no documentation yet, but it will be added soon. (This code was initially intended as a prototype but ended up becoming production-ready — surprise.)

**Note**: This allocator is experimental. Test thoroughly before production use. Benchmark your specific workload.
