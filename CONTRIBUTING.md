# Contributing to Oxidalloc

Thanks for your interest in contributing — seriously, this allocator is wild, and helping out is always appreciated.

## How to Contribute

### 1. Fork the repository

Classic GitHub flow:

* Fork
* Clone your fork
* Create a feature branch

### 2. Build the project

```bash
cargo build --release
```

### 3. Run tests (when tests exist)

Currently Oxidalloc is under heavy development, so test coverage may be limited. Feel free to add new tests.

```bash
cargo test
```

### 4. Code Style

* Keep code readable (Rustfmt recommended)
* Avoid unnecessary complexity — simple fast paths > clever but unreadable logic
* Use comments for non-obvious unsafe sections
* Safety matters: document every unsafe {} block with a justification (except trivial cases like simple pointer dereferences inside well-defined, non-failing functions).

### 5. Making Changes

* Keep commits focused
* If modifying allocator internals, ensure behavior stays ABI-compatible with glibc’s malloc
* Add docs or comments when changing architectural behavior

### 6. Submitting PRs

* Open a Pull Request with a clear description
* Explain what you changed and why
* Include benchmark changes if relevant
* If you found a bug, include reproduction steps

## Performance Contributions

Oxidalloc is performance-sensitive. If your PR affects hot paths:

* Include benchmark data
* Avoid adding overhead to thread-local fast paths
* Keep cross-thread synchronization minimal

## Bug Reports

If you encounter a bug, include:

* Steps to reproduce
* Logs (dmesg/system logs if allocator crashed a system-wide preload)
* Environment (distro, kernel version)
* Whether you used LD_PRELOAD or just Rust global allocator

## Testing Oxidalloc

If you're testing in system-wide mode:

* Keep a recovery ISO or rescue environment handy
* Always test in a VM before bare metal

## License

By contributing, you agree your contributions will be licensed under the project’s MIT license.
