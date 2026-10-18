# Combined cloud results and review limitations

Production base/latest verified main: `3c355f152d405a201482d53b6e45497df4f44a7f`. Source implementation: `e369dee3`. Canonical implementation: `eb7a9be6c960e71e8626134feff4dae24cd1d3bb`. The PR and CI verdict record the final documentation/evidence head separately. Optimized binary hashes and fixed compiler/hardware inputs are in the archived identity files.

All validation samples passed. Baseline and Source-only each have three interleaved pairs per case. Canonical incremental and combined revision comparisons have **six** balanced pairs per case; marginal append has six pairs at each history size. The final Pi comparison has three pairs per case. An additional three revision/append pairs were collected because initial wall results varied; none of the first three or subsequent slow samples was discarded.

**The results are mixed.** Marginal append at larger histories uses less time/CPU and fewer process-attributed writes. Source index file/connection work falls substantially. Process RSS generally increases; memory reduction is not demonstrated. Canonical-only short-small request wall regresses in this matrix, despite a much smaller CPU difference. The independent combined matrix does not reproduce that magnitude. Its cause remains unresolved; it must not be erased or explained away as proven noise. These measurements do not establish a universal whole-request speedup.

All absolute values below are medians of per-run values. Ratios are medians of each matched after/before pair, **not** ratios of unrelated medians. Those two summaries can differ, including in direction. Lower ratios mean less observed cost.

## Stage separation

- [Baseline/profile](baseline.md): unchanged #480 production plus reconstructed fair harness; no desktop measurements reused.
- [Source-only](source.md): baseline versus bounded small-Source SQLite indexes; three pairs.
- Canonical incremental: Source checkpoint versus Source plus canonical change, below; six pairs. This is not a canonical-only production branch.
- Combined: unchanged production baseline versus both changes, below; six pairs.

## Marginal canonical append

Only public canonical acceptance is timed: 32 appends after seeding, with no provider/context construction. Complete ordered history is checked after close/reopen. CPU/write deltas span these 32 appends; lifecycle evidence separately includes seed, close/checkpoint/reopen/cleanup.

| History | Before ms/append | After ms/append | Paired wall ratio | CPU ratio | Write ratio | Sampled RSS before/after MiB |
|---|---:|---:|---:|---:|---:|---:|
| 0 | 0.948 | 0.940 | 1.0637 | 1.4167 | 1.0000 | 27.49 / 31.10 |
| 64 | 0.950 | 0.947 | 0.9826 | 0.8333 | 0.9971 | 27.49 / 31.43 |
| 512 | 1.351 | 1.115 | 0.8488 | 0.7500 | 0.8836 | 28.54 / 32.46 |
| 2048 | 2.496 | 1.964 | 0.7732 | 0.8036 | 0.6397 | 28.82 / 32.60 |

Empty-history CPU ratios are coarse: 10 ms counters over only 32 appends. The empty-history paired wall/CPU regression is retained. At 2,048 messages the paired wall/CPU/write ratios are 0.7732/0.8036/0.6397; that is a marginal-append result, not whole-request O(1). The integrity scan remains O(n).

An illustrative retained 2,048-history pair (`canonical-append-paired/0-2048-*`, after cleanup) has PSS_Anon 6,496→6,448 KiB but PSS_File 22,502→26,418 KiB. Thus its RSS increase is predominantly file-backed residency, not evidence of increased heap allocation. This is an observed decomposition of one pair, not an allocation count or a general memory-saving claim.

## Canonical incremental runtime

| Case | Before turn ms | After turn ms | Paired wall ratio | CPU ratio | Lifecycle write ratio | Sampled RSS before/after MiB |
|---|---:|---:|---:|---:|---:|---:|
| short-small | 70.33 | 92.48 | 1.3334 | 1.0157 | 1.0003 | 201.01 / 203.52 |
| short-large | 77.40 | 81.24 | 1.0444 | 1.0427 | 0.9965 | 285.30 / 286.64 |
| stream | 1630.68 | 1619.38 | 0.9911 | 0.9383 | 1.0003 | 156.17 / 158.13 |
| large | 221.81 | 216.03 | 0.9518 | 0.9792 | 1.0000 | 156.41 / 158.87 |
| tool | 118.98 | 121.27 | 1.0183 | 0.9667 | 0.9998 | 157.57 / 159.68 |
| cancel | 57.73 | 59.02 | 1.0226 | 0.9444 | 0.9996 | 154.97 / 157.66 |

## Combined runtime versus baseline

| Case | Before turn ms | After turn ms | Paired wall ratio | CPU ratio | Lifecycle write ratio | Sampled RSS before/after MiB |
|---|---:|---:|---:|---:|---:|---:|
| short-small | 86.92 | 79.41 | 0.9454 | 0.9838 | 0.9994 | 200.76 / 204.24 |
| short-large | 84.20 | 84.78 | 0.9204 | 0.9860 | 0.9960 | 285.04 / 288.38 |
| stream | 1610.36 | 1617.35 | 0.9988 | 0.9366 | 0.9963 | 155.76 / 158.11 |
| large | 218.02 | 218.08 | 1.0091 | 0.9392 | 0.9997 | 156.37 / 160.23 |
| tool | 131.86 | 125.74 | 0.9536 | 0.8787 | 0.9993 | 156.58 / 159.84 |
| cancel | 55.93 | 54.44 | 0.9434 | 0.9375 | 0.9996 | 154.68 / 157.27 |

## Combined rustX versus Pi Durable

Both real production adapters use the same local HTTP/SSE provider. Pi is pinned to `90d863f7c1183b965f099855715d6e88c7b8a52c`, Node 24.19.0, actual Durable SQLite. Rust uses the same locked optimized Rust 1.98.1 build. Seven rustX native tool schemas and four Pi native schemas remain explicit; paired comparison proves complete normalized **core** request equality, not full schema or durability/feature parity. Old in-process ratios are not equivalent comparisons.

| Case | rustX/Pi turn ms | rustX/Pi CPU ms per turn | rustX/Pi lifecycle writes MiB | rustX/Pi sampled RSS MiB |
|---|---:|---:|---:|---:|
| short-small | 85.16 / 4.84 | 107.81 / 8.75 | 127.89 / 8.88 | 202.72 / 148.79 |
| short-large | 80.59 / 5.82 | 114.38 / 10.31 | 329.14 / 20.79 | 289.43 / 197.74 |
| stream | 1616.64 / 1721.51 | 230.00 / 540.00 | 6.23 / 0.89 | 158.82 / 139.86 |
| large | 224.62 / 114.82 | 240.00 / 140.00 | 13.71 / 0.67 | 160.52 / 113.25 |
| tool | 119.31 / 133.25 | 150.00 / 150.00 | 8.70 / 0.94 | 159.50 / 128.37 |
| cancel | 63.22 / 122.48 | 70.00 / 140.00 | 4.70 / 0.62 | 156.70 / 127.60 |

## Startup, verification and storage scopes

Short-small seeds eight turns, short-large 64; their reported turn values average the next 32 turns. Other cases measure one first turn and include cold adapter behavior. OS cache was warm, never dropped. Formal runs did not overlap builds, tests, profiling or compression.

For final short-small samples, the following are median explicit phase/control measurements; they are not subtracted from unrelated medians:

| Engine | Product startup ms | Provider startup ms | Provider teardown/control ms | Live verification wall/CPU ms | Reopen verification wall/CPU ms |
|---|---:|---:|---:|---:|---:|
| rust | 4.57 | 71.35 | 25.22 | 447.87 / 440.00 | 450.74 / 450.00 |
| pi | 360.06 | 68.82 | 17.10 | 22.30 / 30.00 | 28.25 / 40.00 |

CPU is self user+system accounting at 100 ticks/second, including driver/capture work, not exclusive engine CPU. Provider is a controller-owned sibling and remains alive through product cleanup/exit; provider reaping cannot enter product counters. Provider CPU is outside product scope; startup/control wall is explicit. Verification phases are directly measured. Sampling and capture-hook overhead inside turns are not independently isolated. Evidence-file writing follows final cleanup counters and is included in controller process-lifecycle wall, not separately isolated from exit. No overlapping elapsed scope is presented as a CPU percentage.

RSS/PSS are maximum **sampled** values, not true peaks. Allocation profiling and exclusive CPU profiling are unavailable. Writes are process-attributed OS counters, not physical device writes. DB/WAL/SHM logical and allocated bytes at lifecycle checkpoints are retained in every raw sample; they must not be confused with cumulative writes.

`/tmp` is tmpfs and `/workspace` is overlay. Separate 10 ms controller sampling of product-owned Source files observed maximum concurrent logical/allocated bytes 53,855/57,344 before and 53,001/53,248 after (430/412 samples). It can miss short-lived files and is not a peak or total-allocation claim. The complete syscall profile, separately, shows index files 442→40 and logical index write bytes 11,845,632→1,966,080. SQL/node construction is unchanged; SQL statement counts were not separately instrumented. Original byte files remain.

## Validation and reproducibility

Local passes: 20 Source tests, 20 focused append tests, 367 durable SQLite tests, five Rust oracle regressions, eight Node oracle tests, nine Python harness tests, formatting and all-target/all-feature Clippy. Two initial corruption fixtures failed at setup under foreign keys and were corrected without weakening rejection assertions; all logs are retained. Initial invalid-ID benchmark failures and baseline smoke controller failures remain in their earlier archives.

Commands are in [the benchmark README](../../benchmarks/runtime/README.md). `--pair-offset 3 --pairs 3` supplied the balanced confirmation pairs in separate directories. `confirmation-harness-identity.json` records exact controller blobs and the invocation/commit timing; original manifest heads are not rewritten. `pack-evidence.py` verifies every archived member against its length/SHA-256 and leaves raw originals intact.

Final exact-head FULL and Merge verification are required and linked in [draft PR #484](https://github.com/Caismis/rustX/pull/484), together with actual final head/main/base SHAs. This report does not assert CI success before that verdict. No merge or auto-merge is authorized.
