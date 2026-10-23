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
with one historical canonical message per conversation. Fixture preparation
uses native per-database identity/allocation/history-seed initialization and one
validated, fsynced catalog publication before measurement. This avoids timing
repeated full-catalog rewrites and post-publication allocation rereads in the
Scheduler scan benchmark; it
does not measure Session creation throughput. The measured driver, foreground
reads and control write use their ordinary native paths. Every measured
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

## Measurement integrity

Read hooks are used only for explicitly registered test probes. The benchmark
records `registered_test_probes`, `retained_test_probes` and `probe_integrity`,
and fails if reading otherwise unregistered conversations creates test probes.
The release result also requires this integrity check. Ordinary native database
opens, schema validation, allocation retention and cancellation remain measured.

The minimal native reproduction uses three real conversations and one full pass:

```sh
RUSTX_SCHEDULER_BENCH_SIZE=3 RUSTX_SCHEDULER_BENCH_PASSES=1 \
RUSTX_SCHEDULER_BENCH_MODE=idle cargo test --offline --release --lib \
  --all-features measure_native_scheduler_reconciliation -- --ignored --nocapture
```

Before isolation, [the guard failed](../benchmarks/runtime/scheduler/probe-count-red.json):
one registered probe became three retained probes. After isolation,
[the same run passed](../benchmarks/runtime/scheduler/probe-count-green.json) with
one registered and one retained probe. These small runs validate the measurement
fixture; their CPU/latency samples are not release evidence.

## Diagnostic measurements (2026-10-23)

The following schema 78 sparse runs used the native historical-seed fixture on
WSL2 kernel `6.18.33.2-microsoft-standard-WSL2`, with 16 logical CPUs and data on
its Linux filesystem. Each observed three complete passes, repaired its injected
unavailable source, finished with all sources available and reported no pass-bound
overrun. They preceded probe isolation: the scan allocated the full per-target
test fixture for every conversation. That overhead is absent from production,
so these records cannot establish the production resource gate.

| Conversations | Cold pass | Warm pass p95 | Foreground read p95 | Steady CPU cores | RSS growth |
| --- | --- | --- | --- | --- | --- |
| [100](../benchmarks/runtime/scheduler/schema78-instrumented-sparse-100.json) | 0.664 s | 0.313 s | 4.31 ms | 0.023 | 3.93 MiB |
| [1,000](../benchmarks/runtime/scheduler/schema78-instrumented-sparse-1000.json) | 7.092 s | 2.967 s | 4.63 ms | 0.103 | 16.78 MiB |
| [10,000](../benchmarks/runtime/scheduler/schema78-instrumented-sparse-10000.json) | 86.040 s | 30.639 s | 5.69 ms | 0.508 | 128.65 MiB |

The 10,000-store fixture took 756.60 seconds to prepare and 207.07 seconds to
measure. Steady logical reads were 17.15 MiB/s and physical reads were zero;
startup physical reads are reported separately in the raw JSON. The 30.639-second
warm result, 0.508-core steady CPU and 128.65-MiB growth are diagnostic observations,
not passed budgets. No budget has been relaxed.

Earlier schema 77 [100-store](../benchmarks/runtime/scheduler/schema77-sparse-100.json)
and [1,000-store](../benchmarks/runtime/scheduler/schema77-sparse-1000.json) records
are retained as pre-integration characterization. The first 100-store attempt
failed on a foreground read timeout; its unchanged repeat passed. Native builds
overlapped the 1,000-store setup/measurement. Those records also include automatic
test probes and do not establish acceptance for schema 78.

## Release status

The corrected optimized scale matrix and the 10,000-store idle run with at least
ten passes and 600 seconds remain required. The resource gate is not accepted
until a run with `probe_integrity: true` passes every fixed budget above.
