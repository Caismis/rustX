# Scheduler S2 performance gate

The supported measurement target is an x86-64 Linux host with local regular-file
storage. WSL2 with the dataset on its Linux filesystem is a recorded host, not a
claim about NTFS or network filesystems. The benchmark reports the actual kernel,
logical CPU count, schema and optimized/debug build status.

## Budgets fixed before measurement

For 10,000 historical conversations with zero Enabled schedules and no pending
Scheduler targets, run the optimized native driver for at least ten complete
passes and ten minutes. Required budgets are:

| Resource | Required limit |
| --- | --- |
| Steady process CPU | 0.5 logical core equivalents (CPU seconds / wall seconds) |
| Logical filesystem reads | 64 MiB/s |
| Physical filesystem reads | 8 MiB/s |
| Warm full-pass p95 | 30 seconds |
| Foreground schedule-read p95 | 150 ms |
| Sampled RSS growth after seeding | 128 MiB |
| Healthy sources | All 10,000 available, with zero pending targets |

The sparse scale matrix uses 100, 1,000 and 10,000 real historical stores,
future Enabled schedules, one unavailable database repaired after the first pass,
and a native control write during traversal. Record startup database-cold-cache
and subsequent warm-cache pass latency, CPU, opens, bytes, sampled memory and
foreground latency. Startup uses per-database fsync and POSIX_FADV_DONTNEED;
this is an advisory database-page eviction, with the catalog still warm. Physical
read counters show how effective the hint was. No system-wide cache reset occurs.

## Reproducible native benchmark

`local_runtime::session_runtime_manager::tests::schedule_benchmark::measure_native_scheduler_reconciliation`
is ignored in ordinary correctness runs. Setup creates native Sessions and stores
and appends one historical canonical message per conversation. Every measured
probe retains native allocation, opens the existing database, validates its file
header, full schema shape and exact identity, reads bounded Scheduler headers,
and closes it. No preopened connection, mocked summary or substitute reader is
used. CPU and I/O are process deltas from Linux procfs; RSS is sampled after
seeding, and foreground control reads share the same native path. Startup and
steady CPU/I/O are recorded separately; the steady window begins after the first
complete observed pass. Percentiles use nearest rank, including the upper sample
for a two-sample warm p95. The release gate requires all inventory sources.

Set RUSTX_SCHEDULER_BENCH_SIZE, RUSTX_SCHEDULER_BENCH_PASSES,
RUSTX_SCHEDULER_BENCH_SECONDS and optional RUSTX_SCHEDULER_BENCH_MODE=sparse.
RUSTX_SCHEDULER_BENCH_JSON names an output file. Setting
RUSTX_SCHEDULER_BENCH_RELEASE_GATE requires an optimized 10,000-store idle run
with at least ten passes and 600 seconds, and fails when any budget above fails.
Use `cargo test --release --lib --all-features measure_native_scheduler_reconciliation -- --ignored --nocapture`.

Characterization runs are evidence, not release acceptance. Results and remaining
failures must be recorded here before claiming this gate has passed. Budgets must
not be relaxed after seeing measurements.
