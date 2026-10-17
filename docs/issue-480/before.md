# Stage A: measured baseline before durability changes

Base: `029e50f45d6a24c1452b5d0768274fdd661cda6f` (merged PR 482).
Only measurement fixtures, scripts and documentation differ in this stage.
Schema 69; protocol is recorded in each runtime sample. SQLite 3.46.0,
DELETE/FULL, default features, ordinary optimized release build. Build/run
commands and interpretation are in `benchmarks/runtime/README.md`.

The preserved ordinary executable is
`/tmp/rustx-480-evidence/before-final-conformance`, SHA-256
`485946ef2704e6ec3849e896420af16915606c2f206da403f706d12bed918ad9`.
The source and lockfile hashes, toolchain, hardware and filesystem are in
`evidence/before/before-final/manifest.json`. This stage's commit identifies the
complete benchmark source, including files untracked when the manifest ran.
Measurements ran sequentially, with fresh processes/stores, warm OS caches,
on the host ext4 NVMe filesystem. No real model APIs were used.

## Ordinary baseline

These are preliminary single-run observations, not final speedup estimates.
Three interleaved pairs follow implementation. The final paired runner uses
a corrected accounting harness on both revisions, as explained below.
All six final samples passed their execution and exact read-back oracles.

| Workload | Measured turn median, ms | Process wall, s |
|---|---:|---:|
| short-small (8 seed + 32 measured) | 133.271 | 5.922 |
| short-large (64 seed + 32 measured) | 126.649 | 34.621 |
| stream | 1700.238 | 1.974 |
| large | 539.385 | 0.816 |
| tool | 213.095 | 0.469 |
| cancel | 125.647 | 0.411 |

Process wall includes seed execution, exact historical request reconstruction,
shutdown, reopen and cleanup. It is not the sum of measured turn latencies.
The large-history read-back cost is retained rather than excluded as an outlier.
Streaming includes deliberate provider pacing. CPU ticks, sampled RSS/PSS,
process I/O and file footprints remain in the raw JSON files.

## Separate instrumentation and comparison

The short-small strace run passed: 3,726 fsync entries (843 database,
1,686 journal, 1,197 other), 100,937 pwrite64 entries. Raw compressed traces,
hashes and count limitations are retained. These are syscall observations,
not physical device operations; traced elapsed time is not a headline result.

The existing native attribution profile passed 25 turns in 3,038 ms. Its
inclusive Source scope measured 502,498 microseconds; request Source work
reported 3,232 native header queries, 1,146 node queries, 4,131 index pages,
7,716,222 index bytes and 1,069 metadata blobs. These counters overlap other
scopes and do not represent independent CPU shares. The complete log and
independent instrumented binary identity are retained. Hardware perf access is
denied by host policy; allocation profiling is unavailable, neither is zero.

The pinned Pi Durable SQLite harness passed twelve fresh-process runs (three
each of short-small, short-large, large and stream), including reopen. Raw
per-turn/CPU/memory/footprint results are under `evidence/before/pi`. Pi uses an
in-process faux provider, different streaming pacing and different durable facts
and inspection contracts. This is a product-work comparison, not equivalence.

## Retained failures and fixture repairs

`smoke` retains the initial stream failure: repeated identical text triggered
the product loop guard and extra requests. The final fixture uses numbered
chunks and verifies the exact full text. `before-trace` retains the failed
inventory that raced a legitimately removed temporary journal. The final
inventory records vanished files with null sizes; `before-trace-final` passed.
Earlier successful runs remain under `before`. Nothing is silently discarded.

No durability behavior or Source representation is changed in Stage A.
The final Stage C comparison rebuilds this exact Stage A revision with a shared
harness overlay: join the stopped relay, then measure a final ordinary writable
open/close and assert no WAL frames remain before cleanup. This closes an
accounting gap when a read-only owner outlives the writer. It changes no turn
workload or frozen-input oracle. Original binaries and results above remain
retained; their shutdown window is not substituted for the corrected comparison.
Pi's preliminary runs above also remain retained; final comparisons use the
explicit fixed-chunk, microtask-scheduled version 2 fixture. See the final
[AFTER report](after.md) for paired evidence and exact hashes.
