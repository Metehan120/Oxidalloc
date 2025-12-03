# Oxidalloc

A pure Rust general-purpose memory allocator designed to be used as a malloc replacement via `LD_PRELOAD`.

## Overview

Oxidalloc is a high-performance allocator written entirely in Rust. It is designed to be ABI-compatible with glibc's malloc family and verified to run system-wide across a full Linux desktop environment.

## Tested on Fedora

## Features

* Pure Rust implementation
* Works under `LD_PRELOAD`
* System-wide compatible (systemd, KDE Plasma, GTK apps, browsers, Wine/Proton, etc.)
* Thread-local fast paths
* Cross-thread frees supported
* Optional debug consistency checks
* Fast: ~50 cycles malloc+free on modern CPUs

## Incompatibilities

* WARNING: Under extreme loads like AI Workloads, you may see fragmentation.
* WARNING: Design only working on 64-BIT systems, incompatible with 32-BIT.
* Incompatible with Firefox for now.

## Architecture

* `malloc.rs` – core allocation logic
* `free.rs` – deallocation path and metadata validation
* `realloc.rs` – resizing logic
* `calloc.rs` – zero-initialized allocation
* `align.rs` – alignment-specific allocators
* `thread_local.rs` – per-thread caches
* `global.rs` – global allocator paths and fallbacks
* `internals.rs` – metadata, size classes, constants
* `trim.rs` – memory trimming and page returns
* `lib.rs` – exported C ABI symbols

## Benchmarks:

| Function | Speed (ns) |
|-----------|--------------|
| malloc (thread-local path)   |  6           |
| free   (thread-local path)   |  5           |

## Usage

### Build

```bash
cargo build --release
```

### System-wide preload **(example, do not attempt may cause system instability)**

```bash
echo "/path/to/liboxidalloc.so" | sudo tee /etc/ld.so.preload
```

### Session-only preload

```bash
export LD_PRELOAD=/path/to/liboxidalloc.so
```

## Known Issues

* When Firefox/Firefox-based browsers starting, UI not loading correctly
* There's no real Trim to OS logic **yet**, will be added soon
* Extremely high memory usage when using Rust Analyzer is no full OS-level trimming logic implemented yet — only partial or debug-only trimming is active.
* May crash some APPS

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
