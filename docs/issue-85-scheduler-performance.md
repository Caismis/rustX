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

## Accepted schema 79 measurements (2026-10-23)

The corrected optimized matrix used the recorded WSL2 kernel, 16 logical CPUs
and native databases on ext4. Every run retained exactly its explicitly
registered probes, repaired its unavailable source where applicable, finished
with all sources available and recorded zero discovery-pass overruns.

| Conversations | Cold pass | Warm pass p95 | Foreground read p95 | Steady CPU cores | Sampled RSS growth |
| --- | --- | --- | --- | --- | --- |
| [100](../benchmarks/runtime/scheduler/schema79-sparse-100.json) | 0.688 s | 0.293 s | 3.82 ms | 0.0229 | 2.70 MiB |
| [1,000](../benchmarks/runtime/scheduler/schema79-sparse-1000.json) | 7.128 s | 2.844 s | 4.04 ms | 0.0995 | 13.36 MiB |
| [10,000](../benchmarks/runtime/scheduler/schema79-sparse-10000.json) | 76.012 s | 28.381 s | 5.09 ms | 0.4871 | 111.10 MiB |

The independent [10,000-store idle release run](../benchmarks/runtime/scheduler/schema79-idle-10000.json)
completed ten passes over 600.010 seconds with `release_gate_eligible: true`,
`release_gate_passed: true` and `probe_integrity: true`:

| Required resource | Measured result | Fixed limit |
| --- | --- | --- |
| Steady CPU | 0.4857 cores | 0.5 cores |
| Steady logical reads | 17.86 MiB/s | 64 MiB/s |
| Steady physical reads | 0 MiB/s | 8 MiB/s |
| Warm full-pass p95 | 28.705 s | 30 s |
| Foreground schedule-read p95 | 5.54 ms | 150 ms |
| Sampled RSS growth | 120.98 MiB | 128 MiB |
| Healthy sources | 10,000; zero Enabled schedules or pending Scheduler targets | All 10,000 |

The cold idle pass took 77.365 seconds. Its setup took 549.957 seconds and is
excluded from the measurement window. Native control mutation during traversal
took 204.27 ms; the fixed foreground latency gate concerns reads. The process
drained its driver, readers, commits and writers successfully after measurement.

## Schema validation diagnosis and regression evidence

The corrected [schema 79 idle baseline](../benchmarks/runtime/scheduler/schema79-baseline-idle-10000.json)
ran ten passes over 626.391 seconds with intact probes and healthy sources. It
failed the original budgets at 31.260 seconds warm p95 and 0.5086 steady CPU
cores. Its sampled RSS growth was 118.79 MiB. The corresponding sparse
[100](../benchmarks/runtime/scheduler/schema79-baseline-sparse-100.json),
[1,000](../benchmarks/runtime/scheduler/schema79-baseline-sparse-1000.json) and
[10,000](../benchmarks/runtime/scheduler/schema79-baseline-sparse-10000.json)
results are retained independently of the accepted matrix.

The native open-cost diagnostic isolates fresh connections to one warm historical
database. Two [baseline](../benchmarks/runtime/scheduler/schema79-open-baseline.json)
[runs](../benchmarks/runtime/scheduler/schema79-open-baseline-repeat.json) measured
2.755/2.795 ms per complete native read and 1.948/1.910 ms in complete schema
validation, with 147 SELECT compilations per validation. The finite read-witness
field-order check repeatedly queried the same table's column positions.

`verify_witness_field_order` now prepares one metadata query and reads each
required table's complete column-position map once per verification. It still
checks every original/witness field and every ordering requirement. The map is
discarded after the check; no schema decision or connection is cached across
opens or databases. Header, full shape, identity, allocation retention,
cancellation and writer ownership checks remain in the ordinary native path.

The [optimized diagnostic](../benchmarks/runtime/scheduler/schema79-open-optimized.json)
measured 2.408 ms per complete native read, 1.620 ms in schema validation and
99 SELECT compilations. This isolated diagnostic locates the saved work; the
accepted full scale matrix and idle release run establish the resource gate.

```sh
RUSTX_SCHEMA_BENCH_ITERATIONS=1000 \
RUSTX_SCHEMA_BENCH_JSON=/path/to/schema-open.json \
cargo test --offline --release --lib --all-features \
  measure_native_schema_open_cost -- --ignored --nocapture
```

Keep the Linux source, build cache and logs in a persistent directory outside
`/tmp`. Archive each small JSON output into the repository immediately after
its run. Temporary native fixture data is released by the benchmark's owners.

## Schema 80 margin diagnosis

The first schema 80 [idle integration result](../benchmarks/runtime/scheduler/schema80-baseline-idle-10000.json)
completed ten passes over 601.944 seconds with intact probes and all 10,000
sources healthy. It failed the unchanged limits at 0.50265 steady CPU cores and
30.018 seconds warm p95. Its foreground read p95 was 5.64 ms and sampled RSS
growth was 119.61 MiB. The accompanying sparse
[100](../benchmarks/runtime/scheduler/schema80-baseline-sparse-100.json),
[1,000](../benchmarks/runtime/scheduler/schema80-baseline-sparse-1000.json) and
[10,000](../benchmarks/runtime/scheduler/schema80-baseline-sparse-10000.json)
records remain separate from the final matrix below.

The [native opening diagnostic](../benchmarks/runtime/scheduler/schema80-open-baseline.json)
measured 2.561 ms per complete read, including 1.674 ms in full schema checking
and 99 SELECT compilations. Merely reusing two index lookup statements reduced
the [compilation count to 72](../benchmarks/runtime/scheduler/schema80-open-prepared-indexes.json)
but did not establish a wall-time improvement (2.597 ms complete, 1.705 ms schema).
That candidate was not retained as a performance fix.

The final verifier queries the finite set of required native index definitions
once, retaining every exact SQL comparison and the presentation indexes' events
owner predicate. Names remain bound parameters. The SQL-byte output cap derives
from the largest accepted definition; a row cap plus duplicate/missing checks
bound the returned set. Other column, order, uniqueness, foreign-key, identity,
header, allocation and cancellation checks remain. No database validation result
is cached across opens or stores.

The [batch diagnostic](../benchmarks/runtime/scheduler/schema80-open-optimized.json)
measured 2.353 ms complete / 1.462 ms schema with 71 SELECT compilations.
Sequential same-host paired repetitions measured complete reads of
[2.52](../benchmarks/runtime/scheduler/schema80-open-prepared-indexes-repeat-1.json) /
[2.54](../benchmarks/runtime/scheduler/schema80-open-prepared-indexes-repeat-2.json) ms
for the prior candidate and
[2.31](../benchmarks/runtime/scheduler/schema80-open-optimized-repeat-1.json) /
[2.26](../benchmarks/runtime/scheduler/schema80-open-optimized-repeat-2.json) ms
for the batch verifier. These isolate the saved work; only the full native
matrix and idle run below establish release acceptance.

The first full idle attempt with this batch verifier
[aborted on a foreground read](../benchmarks/runtime/scheduler/schema80-idle-lock-failure.txt)
after three complete passes at `read journal mode: database is locked`.
SQLite documents the last connection's short exclusive cleanup lock among
[WAL busy cases](https://sqlite.org/wal.html#sometimes_queries_return_sqlite_busy_in_wal_mode).
The native ScheduleReadScope's zero busy wait also reproduced that exact error
against one exclusively locked WAL writer in a seconds-long standalone loop.
The transient-lock regression failed before the fix. Readers now use SQLite's
25 ms bounded busy timeout, retaining the manager's overall read deadline,
interrupt/progress cancellation and finite VM budget. Transient writer release,
persistent-lock cancellation, running-query cancellation and VM-budget fixtures
all pass. The final matrix below was rerun after this production change.

## Schema 80 integration acceptance (2026-10-24)

The final integration includes origin/main `00978bbe3` (native Workflow Model
nodes and parallel source preparation), App Server v56 and Runtime Client v75.
The complete scale matrix and independent idle gate were rerun with the final
schema 80 source and optimized native benchmark. No other task build/test ran
during these measurements. Kernel `6.18.33.2-microsoft-standard-WSL2`, 16
logical CPUs, native stores on WSL2 ext4.

| Conversations | Cold pass | Warm pass p95 | Foreground read p95 | Steady CPU cores | Sampled RSS growth |
| --- | --- | --- | --- | --- | --- |
| [100](../benchmarks/runtime/scheduler/schema80-sparse-100.json) | 0.642 s | 0.264 s | 3.66 ms | 0.0211 | 3.25 MiB |
| [1,000](../benchmarks/runtime/scheduler/schema80-sparse-1000.json) | 5.901 s | 2.571 s | 3.75 ms | 0.0905 | 12.92 MiB |
| [10,000](../benchmarks/runtime/scheduler/schema80-sparse-10000.json) | 72.666 s | 25.540 s | 4.82 ms | 0.4722 | 112.63 MiB |

The [idle release result](../benchmarks/runtime/scheduler/schema80-idle-10000.json)
completed 10 passes over 600.148 seconds with
intact registered probes, all 10,000 sources healthy, no discovery overrun,
and `release_gate_eligible: true` / `release_gate_passed: true`.

| Required resource | Measured result | Fixed limit |
| --- | --- | --- |
| Steady CPU | 0.4489 cores | 0.5 cores |
| Steady logical reads | 17.64 MiB/s | 64 MiB/s |
| Steady physical reads | 0.00 MiB/s | 8 MiB/s |
| Warm full-pass p95 | 26.004 s | 30 s |
| Foreground schedule-read p95 | 5.21 ms | 150 ms |
| Sampled RSS growth | 119.30 MiB | 128 MiB |

The idle cold pass took 70.769 seconds;
setup took 346.476 seconds, outside the measurement window.
Native control mutation during traversal took 197.10 ms.
All runs completed native driver/reader/commit/writer drain assertions.

## Schema 81 final execution and authority acceptance

The final source adds durable Scheduled Workflow start/effect/settlement storage,
real Scheduled execution and interaction scopes, bounded physical recovery and
current-Human model authority. Formats are App Server 57, Runtime Client 76,
SQLite 81 and child IPC 35, with catalog 18. The complete sparse matrix and
independent idle gate were measured again on the recorded WSL2 ext4 host with
all original budgets. No other task build or test ran during measurement. These measurements were
recorded at `366aa2c29`. Subsequent fork/main `006d1bb4a` integration changes
file previews and App Server transfer/error handling; it changes none of the
measured Scheduler storage, discovery, execution or manager read paths.

| Conversations | Cold pass | Warm pass p95 | Foreground read p95 | Steady CPU cores | Sampled RSS growth |
| --- | --- | --- | --- | --- | --- |
| [100](../benchmarks/runtime/scheduler/schema81-sparse-100.json) | 0.643 s | 0.290 s | 3.70 ms | 0.0218 | 2.69 MiB |
| [1,000](../benchmarks/runtime/scheduler/schema81-sparse-1000.json) | 6.816 s | 2.715 s | 3.89 ms | 0.0934 | 12.16 MiB |
| [10,000](../benchmarks/runtime/scheduler/schema81-sparse-10000.json) | 80.073 s | 27.040 s | 5.00 ms | 0.4827 | 112.00 MiB |

The [schema 81 idle release result](../benchmarks/runtime/scheduler/schema81-idle-10000.json)
completed 10 passes over 600.170 seconds. Registered probes were intact,
all 10,000 sources were healthy with zero pending targets, and there was no pass
overrun. Both release_gate_eligible and release_gate_passed are true.

| Required resource | Measured result | Fixed limit |
| --- | --- | --- |
| Steady CPU | 0.4696 cores | 0.5 cores |
| Steady logical reads | 19.20 MiB/s | 64 MiB/s |
| Steady physical reads | 0.00 MiB/s | 8 MiB/s |
| Warm full-pass p95 | 27.247 s | 30 s |
| Foreground schedule-read p95 | 5.23 ms | 150 ms |
| Sampled RSS growth | 119.35 MiB | 128 MiB |

The cold pass took 77.387 seconds; native data setup took
551.261 seconds outside the measurement window. The native control write
during traversal took 241.68 ms. Driver, reader, commit and writer
retirement assertions all completed. Kernel and CPU topology remain recorded in
each raw result. This is evidence for local Linux ext4, including this WSL2 host;
it does not establish NTFS, network-filesystem or macOS resource acceptance.

## Release status

The native scale/idle gate is accepted for the final schema 81 source on the
recorded host with unchanged budgets. Earlier schema 79/80 raw results and their
failure/optimization diagnosis remain preserved above. Durable Workflow execution,
recovery and current-Human authority are implemented and covered by the
[current integration validation](issue-85-scheduler-validation.md). Platform and
product soak coverage outside that report remain separate release evidence.
