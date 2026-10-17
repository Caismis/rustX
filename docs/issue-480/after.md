# Stage C: WAL/NORMAL runtime evidence

> Historical pre-review evidence. Review of `426bfe57` found that request checks
> did not prove complete ordered wire/frozen input and ordinary CPU/I/O samples
> omitted initialization and post-verification phases. Wall windows and the
> separate syscall trace already included deferred maintenance. All records below
> remain retained; [revised acceptance evidence](review-repair.md) is authoritative
> for the corrected oracle and lifecycle CPU/I/O.

## Reproducibility and limits

Baseline code is Stage A `c34f87983568700c7dfa2416722cdd2cd0fd9315`, based on dependency-complete main `029e50f45d6a24c1452b5d0768274fdd661cda6f`. AFTER code is Stage B `b2b664e1`. The baseline archive receives only the identical final benchmark harness overlay. [Build provenance](evidence/after/build-provenance.json) records full hashes, commands and the original preserved baseline binary. The overlay joins the stopped relay and measures a final ordinary writable open/close before cleanup, proving no WAL frames remain. It changes no turn workload or frozen-request oracle.

Both ordinary binaries use default-feature optimized Rust 1.98.1 builds, on the same i9-14900K host (32 logical CPUs, approximately 125 GiB RAM), ext4 NVMe filesystem, warm OS cache and fresh processes/stores. No task-owned build/test runs concurrently with headline measurements. BEFORE uses SQLite 3.46.0 DELETE/FULL; AFTER uses SQLite 3.53.2 WAL/NORMAL. The dependency upgrade is part of this change, so this is not an isolated PRAGMA experiment. No real model API or credentials are used.

Three interleaved BEFORE/AFTER pairs cover every case. The first large-response BEFORE run failed after 5.446 seconds with `presentation frontier failed` at the driver’s original attempt Journal read; no successful sample JSON was produced. The precise underlying SQLite error is not exposed by that API. All 18 initial AFTER samples passed. One additional large-response pair passed, providing three complete pairs for that case. The failed process/log and the unmatched successful AFTER sample remain included in the evidence. Overall: 18/19 BEFORE executions and 19/19 AFTER executions passed. These small samples do not establish reliability rates.

No slow sample is removed: short-small BEFORE process wall spans 5.91–25.41 seconds; stream BEFORE includes a 5.31-second turn; the first AFTER large-response turn is 619.56 ms. Correctness-only smoke runs ran during builds and are excluded from performance statistics by design. The original Stage A/Pi preliminary results remain separate, with accounting/fixture corrections documented in [BEFORE](before.md).

## Ordinary execution

Values are medians of per-run measured-turn medians; brackets show the full range of those per-run medians. Whole-process wall includes seed turns, exact historical reconstruction, shutdown, final writer close, canonical reopen and cleanup. Pair ratio is AFTER/BEFORE for complete pairs only; all successful observations contribute to the individual medians. Large has three successful BEFORE and four AFTER observations.

| Workload | BEFORE turn ms [range] | AFTER turn ms [range] | Complete-pair ratio median | Process wall s BEFORE → AFTER |
|---|---:|---:|---:|---:|
| short-small | 128.78 [127.24–140.77] | 65.34 [65.11–71.75] | 0.507 | 6.030 → 3.354 |
| short-large | 128.88 [128.79–137.05] | 77.42 [74.03–80.07] | 0.575 | 14.865 → 9.067 |
| stream | 1717.66 [1647.11–5308.16] | 1612.28 [1581.46–1644.96] | 0.939 | 2.018 → 1.913 |
| large | 580.25 [553.95–610.85] | 204.60 [185.44–619.56] | 0.350 | 0.884 → 0.475 |
| tool | 275.45 [224.30–2430.45] | 117.68 [101.77–127.40] | 0.369 | 0.565 → 0.388 |
| cancel | 126.69 [122.15–218.64] | 64.35 [50.23–67.31] | 0.527 | 0.427 → 0.357 |

Short-small measured turn latency falls about 49%; short-large about 40%. Streaming is largely governed by explicit provider gates. First-consumed publication for short-small falls from 98.61 to 53.23 ms and short-large from 99.45 to 65.66 ms. Large-response first-consumed publication instead rises from 101.21 to 173.04 ms despite faster settlement. This consumer-side metric is not first emission latency; stream/cancel notifications are drained only after their gates, so their publication figures must not be compared with short replies. No allocation or SQL-statement count is fabricated.

| Workload | CPU ms/turn BEFORE → AFTER | Sampled max RSS MiB BEFORE → AFTER | Max live logical MiB BEFORE → AFTER | Max live allocated MiB BEFORE → AFTER | Final writer open/close ms BEFORE → AFTER |
|---|---:|---:|---:|---:|---:|
| short-small | 126.88 → 98.44 | 179.41 → 178.50 | 9.21 → 13.81 | 9.22 → 13.82 | 4.12 → 9.18 |
| short-large | 128.12 → 109.69 | 199.51 → 203.80 | 19.69 → 24.14 | 19.71 → 24.16 | 4.14 → 12.51 |
| stream | 300.00 → 260.00 | 155.35 → 156.00 | 1.39 → 4.21 | 1.40 → 4.23 | 7.99 → 16.31 |
| large | 310.00 → 225.00 | 157.35 → 157.67 | 1.62 → 5.48 | 1.63 → 5.50 | 4.38 → 9.29 |
| tool | 210.00 → 140.00 | 156.66 → 156.27 | 1.91 → 5.83 | 1.92 → 5.85 | 7.28 → 9.35 |
| cancel | 100.00 → 80.00 | 155.20 → 154.83 | 1.33 → 2.73 | 1.34 → 2.74 | 7.72 → 11.44 |

CPU is process user+system tick accounting divided by measured turns, including driver/sampling work and excluding the Python provider. RSS/PSS are sampled, not peaks; full PSS and process-write-byte values are in the summaries. Neither CPU nor filesystem accounting measures allocations or physical media traffic.

WAL raises live footprint. The 96-turn short-large workload ends with approximately 4.59 MiB of WAL; its final writer open/close median is 12.51 ms (range includes a 204.93 ms sample). The 40-turn short-small final writer close median is 9.18 ms. These windows include open validation as well as checkpoint/close and are not exclusive checkpoint timers. Every successful sample asserts zero remaining WAL frames before cleanup. Automatic checkpoints remain at SQLite’s default 1,000-page threshold; no critical-path forced checkpoint was introduced. Scoped snapshot lifetimes are separately validated by the archive backpressure/TRUNCATE test.

| Workload | Unload ms BEFORE → AFTER | Driver drop ms BEFORE → AFTER | Cleanup ms BEFORE → AFTER |
|---|---:|---:|---:|
| short-small | 7.670 → 6.763 | 0.280 → 0.328 | 1.186 → 1.190 |
| short-large | 13.264 → 13.247 | 0.342 → 0.320 | 2.787 → 2.384 |
| stream | 1.610 → 2.285 | 0.264 → 0.289 | 0.398 → 0.602 |
| large | 2.157 → 1.475 | 0.176 → 0.307 | 0.442 → 0.423 |
| tool | 2.004 → 1.527 | 0.249 → 0.297 | 0.476 → 0.502 |
| cancel | 1.323 → 1.180 | 0.284 → 0.233 | 0.371 → 0.390 |

## Separate syscall evidence

The short-small traced pair passed both oracles. Timing from strace is excluded above. The parser joins unfinished/resumed calls; both traces have zero unresolved calls. Raw gzip traces and returned-byte summaries are retained. Trace scope includes runtime/driver and provider/helper children; database filenames separate SQLite traffic from other files/pipes/sockets.

| Completed calls | BEFORE | AFTER |
|---|---:|---:|
| fsync | 3,726 | 175 |
| pwrite64 | 100,871 | 70,154 |
| fcntl | 325,784 | 187,101 |
| unlink | 843 | 139 |

Successful sync calls decline from 3,726 to 175 (95.3%). BEFORE includes 843 database, 1,686 journal and 1,197 other syncs; AFTER includes 27 database, 52 WAL, one journal and 95 other syncs. These include deferred checkpoint/close work. Two AFTER `fcntl(F_SETLK, F_WRLCK)` calls returned EBADF on conversation database descriptors; the trace retains them, the application oracle passed, and there were no failed traced writes or sync calls. This does not equate every failed lock attempt with a failed committed write.

| Successful returned write bytes by path | BEFORE | AFTER |
|---|---:|---:|
| database | 117,825,536 | 40,960,000 |
| journal | 96,774,924 | 524 |
| wal | 0 | 104,685,912 |
| shm | 0 | 32 |
| sqlite_temp | 0 | 27,256,800 |
| other | 1,525,997 | 1,549,052 |

AFTER exposes 27,256,800 bytes of `etilqs_` temporary-file writes, absent in this BEFORE trace. They are included, not folded into a claim of zero overhead; sequential live-allocation inventories need not capture transient files outside that allocation. Database/journal/WAL/SHM/temp returned writes total roughly 214.6 MB BEFORE and 172.9 MB AFTER. This is syscall byte accounting, not physical device writes or SQL/transaction counts. The SQLite upgrade and changed concurrency are confounded variables; no unsupported causal attribution is made.

## Native attribution and remaining work

All six separately instrumented 25-turn native profile runs passed. Test-reported elapsed milliseconds were BEFORE `[5385, 15199, 2827]` and AFTER `[4439, 1490, 1520]`; these variable, instrumented values are not the ordinary headline timing. Inclusive Source milliseconds were BEFORE `[445.747, 455.820, 454.360]` and AFTER `[446.229, 427.050, 450.836]`. Scopes overlap and are not independent CPU percentages. Publication record counts vary with consumer scheduling and are preserved in the logs.

Every run reports the same request Source work: 3,232 header queries, 1,146 node queries, 4,131 index pages, 7,716,222 index bytes, 1,069 metadata blobs and 1,267,493 metadata bytes; 51 new leaves, 190 new nodes, 405,789 new fragment bytes and zero header decodes. This change removes synchronization cost without reducing that work. Source representation/CPU optimization remains reserved for #481. Hardware perf is denied by host policy and no allocation profiler is installed; neither unavailable measurement is zero.

## Pinned Pi Durable SQLite comparison

All twelve version-2 repeats passed using Pi `90d863f7c1183b965f099855715d6e88c7b8a52c`, its actual durable harness, SQLite backend, lockfile dependencies and in-process faux provider. Runtime is Node 24.19.0 with SQLite 3.53.3 (full versions are recorded per run). Fixed unpaced chunks are 256 characters via microtasks; streaming uses 16-character chunks at 425 tokens/s. RustX uses real HTTP/SSE, tool execution and richer retained facts/Source/inspection contracts, with 17-character streaming chunks gated every 10 ms. This is a comparison of actual product work, not feature/durability parity or an isolated storage benchmark.

| Workload | Pi measured turn median ms | Pi measured CPU ms/turn | Pi max live logical MiB | Pi explicit close ms | rustX AFTER turn median ms |
|---|---:|---:|---:|---:|---:|
| short-small | 1.587 | 2.366 | 4.125 | 2.319 | 65.338 |
| short-large | 1.391 | 2.016 | 4.207 | 2.253 | 77.416 |
| stream | 1338.985 | 56.727 | 0.605 | 5.449 | 1612.276 |
| large | 9.998 | 11.544 | 0.436 | 4.040 | 204.600 |

Pi remains much faster on short turns. WAL/NORMAL alone does not close the gap; the unchanged native Source work and different retained contracts make #481 a separate measured optimization, not an excuse to alter this PR’s semantics.

## Correctness and delivery

[Validation](validation.md) records 4,905 passing local Rust tests across the required lanes, 55 Python tests, focused WAL/crash/archive/Fork proofs, formatting/Clippy/lane checks, and retained failed attempts with their diagnoses. The [durability contract and finite synchronization audit](durability.md) defines process-crash guarantees, explicitly excludes newest-commit machine-failure durability, and preserves distinct P/U/C commits, publication order, frozen requests, artifact seals/witnesses and same-buffer export verification. Remote FULL CI and independent review remain mandatory on the final head. No merge is authorized; #481 remains blocked until #480 is reviewed and merged.
