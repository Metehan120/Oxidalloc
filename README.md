## Rewriting the Allocator
This version of Oxidalloc works, but the internal design has reached its practical limits.

After a lot of profiling, testing, and real-world use, it became clear that the current structure is not ideal for long-term performance, fragmentation control, or feature growth.

**I’ve decided to rewrite the allocator from scratch.**

### The new design will focus on:
- less fragmentation
- faster allocation paths
- proper physical + virtual page lifecycle
- cleaner internal invariants
- easier future extensions

This rewrite is not a patch.

## **Current Rewrite Status**: Mostly complete. The rewrite reuses core logic from the previous version, but is significantly simplified, safer, and more efficient.
## **Current goal on Rewrite**: Figure out how to implement Trim on rewritten version and add documentation meanwhile.

# Oxidalloc

A pure Rust general-purpose memory allocator designed to be used as a malloc replacement via `LD_PRELOAD`.

## Overview

Oxidalloc is a high-performance allocator written entirely in Rust. It is designed to be ABI-compatible with glibc's malloc family and verified to run system-wide across a full Linux desktop environment.

## Tested on Fedora

## Features

* Pure Rust implementation
* Works under `LD_PRELOAD`
* Thread-local fast paths
* Cross-thread frees supported
* Optional debug consistency checks
* Fast: ~60 cycles malloc+free on modern CPUs (10ns on 4.65ghz) 

## Incompatibilities
* WARNING: Design only working on 64-BIT systems, incompatible with 32-BIT.
* Incompatible with Firefox for now.
* Under some certain conditions, it may cause fragmentation.

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

* Not compatible with Firefox.
* High memory usage when using Rust Analyzer. | No trim yet, thats why.
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
