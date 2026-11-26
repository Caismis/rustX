# Current runtime performance baseline

This document is the accepted current-HEAD runtime baseline for the Phase 1 measurement-credibility pass. It deliberately excludes historical candidate experiments and schema-versioned results that do not measure the current tree.

## Common measurement identity

Unless a section says otherwise, measurements used:

- Source baseline: `b90398338dd27b91d6e4e8c9bce1a8a7656ce432` (`main` when Phase 1 started). The Phase 1 patch changes measurement harnesses and reporting only; it does not change runtime product logic.
- Durable schema: 114.
- Runner: 8 logical CPUs, 31 GiB RAM, Linux kernel `6.8.0-1060-aws`.
- Rust toolchain: `rustc 1.100.0 (0ef1bc386 2026-11-09)`; Cargo `1.100.0`.
- Release product binary SHA-256: `93c7f531e4983b716e44443e2ab9e865bd1c6b0c6ece9fb1648be5d9a656926d` (`target/release/rustx`).
- Node: `v24.22.3`; Node binary SHA-256 `a7ec1b37e6ff9d187e4198c5b6df9a4eb1c74dd31804fb3bac55cc4ceca1ac01`; pnpm `11.13.1`.
- No compile or unrelated test ran concurrently with an accepted timing measurement.

## Web incremental rendering counters

Measurement: `cd web-console && RUSTX_PERFORMANCE_OUTPUT=<output> pnpm test:issue-420-performance` for fixture `200-long-reasoning-tool-v3`.

- Input fixture aggregate SHA-256: `5f872fb8f60d5e90def34585098b355c486f9352798780878f1b68e531e7eb13` over `test/incremental-performance.measurement.tsx`, `test/fixture.ts`, and `vite.performance.config.ts`.
- Concurrency/load: one jsdom Vitest file, `maxWorkers: 1`, `fileParallelism: false`; in-process fixture runtime, no native App Server or browser.
- Samples: 3 independent successful Vitest process runs after repair. The workload is deterministic and all three runs produced the identical counter vector, so the median and worst sample are the same vector.
- Assertion kind: exact deterministic structural counters only; no wall-clock threshold assertion.

Accepted counter vector:

| fixture | snapshots | commits | automaticWrites | bottomWrites | anchorWrites | rowReplacements |
|---|---:|---:|---:|---:|---:|---:|
| `200-long-reasoning-tool-v3` | 1 | 364 | 100 | 101 | 0 | 0 |

Repair contrast: before the fixture repair the command exited 1 with six unhandled `Fixture needs an explicit native result for conversation/contentRead` errors and printed `snapshots: 1, commits: 412`; because those errors invalidated the run, the pre-repair vector is diagnostic only and is not an accepted baseline. After repair the command exits 0 with zero unhandled errors and the accepted vector above.

## Turn-growth smoke, schema 114

Measurement: `benchmarks/runtime/turn-growth.py`, 128 sequential turns per scenario on one conversation, with 32-turn windows. Raw normalized summary: `benchmarks/runtime/turn-growth/schema114-smoke-128.json`.

- Input fixture aggregate SHA-256: `2470a78f4c24da3bb6c8a66eb485c75412c0b58aba07e500da5e58a9eef53732`.
- Protocol fixture hash coverage: the turn-growth driver and oracle, capacity helper, `Cargo.lock`, and all fake-provider source files listed in the raw summary.
- Binary SHA-256: `93c7f531e4983b716e44443e2ab9e865bd1c6b0c6ece9fb1648be5d9a656926d`.
- Concurrency/load: one active conversation at a time; turns are sequential. The fake provider and driver are sibling processes. The server was pinned with the driver to CPUs 2-3 and used 4 Tokio worker threads. No compile or unrelated test ran concurrently.
- Samples per scenario: 128 turns. The first turn is a cold-outlier sample and is reported rather than excluded; an additional post-first-turn worst sample is shown for interpretation.
- Scope: this is a smoke baseline, not a release gate and not proof of asymptotic behavior. CPU/I/O include settle and retirement reads; settled/idle latency excludes between-window settle. OS file cache remained warm.

| scenario | samples | settled median ms | worst settled ms | worst settled after turn 1 ms | idle median ms |
|---|---:|---:|---:|---:|---:|
| text | 128 | 18.996 | 85.059 (turn 1) | 40.264 (turn 52) | 20.620 |
| tool | 128 | 50.967 | 140.666 (turn 1) | 99.937 (turn 128) | 53.200 |

Window means:

| scenario | turns | wall ms/turn | CPU ms/turn | write bytes/turn | SQLite main MiB | WAL MiB |
|---|---|---:|---:|---:|---:|---:|
| text | 1-32 | 20.282 | 46.875 | 209,920 | 1.42 | 4.02 |
| text | 33-64 | 19.458 | 44.375 | 226,944 | 2.45 | 4.02 |
| text | 65-96 | 20.474 | 46.875 | 236,416 | 3.47 | 4.02 |
| text | 97-128 | 19.905 | 45.312 | 311,296 | 5.45 | 4.02 |
| tool | 1-32 | 55.054 | 103.750 | 475,520 | 2.43 | 4.02 |
| tool | 33-64 | 51.591 | 94.688 | 595,584 | 5.37 | 4.03 |
| tool | 65-96 | 52.540 | 95.625 | 619,008 | 8.28 | 4.03 |
| tool | 97-128 | 58.329 | 103.125 | 738,688 | 12.08 | 4.05 |

Linear window-midpoint slopes are smoke indicators only:

| scenario | wall ms/history turn | CPU ms/history turn | write bytes/history turn | SQLite main bytes/history turn |
|---|---:|---:|---:|---:|
| text | -0.000363 | -0.006836 | +980.000 | +42,960.000 |
| tool | +0.033665 | -0.002930 | +2,540.400 | +104,374.400 |

Reopen copies in fresh server processes:

| scenario | history turns | attach wall ms | subscribe wall ms | RSS growth MiB | PSS growth MiB |
|---|---:|---:|---:|---:|---:|
| text | 32 | 78.519 | 13.686 | 28.34 | 17.89 |
| text | 128 | 135.379 | 9.787 | 27.34 | 17.19 |
| tool | 32 | 148.618 | 19.222 | 29.81 | 18.40 |
| tool | 128 | 174.134 | 29.016 | 29.01 | 17.50 |

## Durable write-path structural counters

Measurement: `cargo test --release --locked -p rustx-durable warmed_turn_write_work_is_fixed -- --nocapture`.

- Input fixture aggregate SHA-256: `c20953371ca31a77cd410d270dfb4eaf5024a949de28ee5429700fa28ce8102d` over `crates/rustx-durable/src/sqlite/turn_write_work_tests.rs` and `Cargo.lock`.
- Test binary SHA-256: `da4a1050bacd614507be2afdaa91af0d474fb674e8f1e44ee791da05b842a688`.
- Concurrency/load: one single-threaded deterministic unit test; no other benchmark ran concurrently.
- Samples: 1 accepted run; result `1 passed`, finished in 0.21 s. These are exact structural counters, not wall-clock measurements, so median and worst are the same counter vector.

Accepted exact vectors:

| scenario | calls | prepares | VM steps | commits | WAL frames | event-index entries |
|---|---:|---:|---:|---:|---:|---:|
| text turn | 617 | 18 | 20,512 | 7 | 263 | 58 |
| tool turn | 1,392 | 38 | 45,633 | 13 | 559 | 112 |

## Scheduler reconciliation release gate, schema 114

Measurement: `RUSTX_SCHEDULER_BENCH_SIZE=10000 RUSTX_SCHEDULER_BENCH_PASSES=10 RUSTX_SCHEDULER_BENCH_SECONDS=600 RUSTX_SCHEDULER_BENCH_RELEASE_GATE=1 cargo test --release --locked --lib --all-features measure_native_scheduler_reconciliation -- --ignored --nocapture`. Raw report: `benchmarks/runtime/scheduler/schema114-idle-10000.json`.

- Input fixture aggregate SHA-256: `299b958479bfee1f970dcb183e3eb8494d7f19d5a841eb22656ce3f1240d8c68` over `tests/scripted/app_server/schedule_benchmark.rs` and `Cargo.lock`.
- Test binary SHA-256: `5710d3e212d70b16ad8b872ddba86920b80706860fd3b2b941b24711cab00289`.
- Release libtest compile time on this runner: 47m01s. The compile finished before measurement; no compile or unrelated test ran during seeding or measurement.
- Concurrency/load: 10,000 zero-enabled, zero-pending durable conversation stores in one process; 4 Tokio worker threads; native historical seed initialization plus one validated durable catalog publication before measurement; foreground control reads ran concurrently with reconciliation passes as designed by the benchmark.
- Setup samples/time: one seeding phase, 250.544 s.
- Pass samples: 10 total passes. The first pass is the cold-cache startup sample; the warm distribution therefore has 9 samples. Median and worst below are explicit, not fastest-only.
- Cache method: fsync plus per-database POSIX_FADV_DONTNEED before startup; catalog metadata remains warm; later passes use ordinary kernel cache.

Accepted current result:

| metric | value |
|---|---:|
| startup cold pass | 82,944 ms |
| warm-pass samples | 9 |
| warm-pass p50 | 84,091 ms |
| warm-pass p95 | 98,497 ms |
| warm-pass worst | 98,497 ms |
| foreground-read samples | 4,203 |
| foreground-read p50 | 11.452 ms |
| foreground-read worst | 93.295 ms |
| write during traversal | 212.170 ms |
| elapsed measurement | 1,112.222 s |
| steady CPU core equivalents | 0.782 |
| database open path entries | 104,205 |
| RSS baseline / peak | 242,745,344 / 379,662,336 bytes |
| pass overruns | 0 |
| unavailable sources | 0 |
| probe integrity | passed |
| release gate eligible | yes |
| release gate passed | **no** |

This is the first complete schema-114 10,000-store result in this audit. It is valid current evidence and **fails the existing release budget**; it must not be replaced by the older schema-81 passing result. This Phase 1 PR records the baseline only and does not infer a root cause or perform speculative optimization.

## Local validation limitations

This VM runs Ubuntu 22.04 with systemd 249. The isolated Office document tests require the newer systemd user-service `ExitType=cgroup` property used by the repository sandbox and supported by current GitHub Ubuntu runners. That environment limitation is separate from the performance baseline and must not be hidden or converted into a skipped product assertion.
