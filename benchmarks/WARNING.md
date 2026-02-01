# WARNING

Most of these tests do not reflect real-world allocator behavior under realistic workloads.
They are synthetic stress tests intended to expose edge cases, tradeoffs, and worst-case
behavior rather than predict application-level performance.

More realistic, application-like behavior can be observed in the `*_cpu` and `*_db`
benchmarks (e.g. `redis`, `rocksdb`, `lua`).
