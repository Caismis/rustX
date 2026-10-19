# Final combination measurements

Measured candidate production head: `99da5d688178227ecb0c75d7db8ac5c73cb3918f`.
SQL reuse was provisionally removed in `218b5043`, then restored in
`ef11f5de` after the bounded balanced comparison passed its predefined rule.
The final production Rust/Cargo/test trees exactly match this measured candidate.
See [final selection and evidence](final-selection.md); earlier contradictory
stage results remain retained.

Base/main: `3c355f152d405a201482d53b6e45497df4f44a7f` (verified ancestor).
Optimized executable SHA-256:
`3757014962aa24e831f8562cfd04d729f4435690d575cba17f9e3a0c2f9cfd55`.

All 192 fresh-process samples passed: four 36-sample runtime matrices, 24
marginal-append samples, and 24 additional cancellation samples. Each runtime
matrix has three alternating matched pairs for every workload. No build or
profiling run competed with formal samples. Lightweight report extraction ran
between stage completions while later matrices continued. Raw failures/slow
observations from all earlier stages are retained, not replaced by these runs.

The same local HTTP/SSE provider serves both production adapters. Pi uses actual
Durable SQLite at `90d863f7c1183b965f099855715d6e88c7b8a52c`, Node 24.19.0.
All ordered expected/wire/captured/frozen/reopened comparisons passed; native
rustX/Pi tool schemas remain explicitly different. Product and provider are
controller-owned sibling processes; provider cleanup follows product cleanup.

CPU is product self user+system ticks, including the driver, at 10 ms resolution;
provider/controller CPU is outside that counter. Evidence output occurs after
cleanup endpoints. Writes are process-attributed, not physical-device writes.
RSS/PSS are sampled, not peaks. Source tmpfs logical traffic is not device I/O.
Complete initialization, seed, verification, close/checkpoint, reopen and cleanup
phases and external provider startup/control costs remain in every raw sample.
Ratios below are medians of paired ratios, never ratios of unrelated medians.

## Compact Source alone (versus SQL stage)

| Workload | Wall | CPU | Sampled RSS | Lifecycle writes |
|---|---:|---:|---:|---:|
| short-small | 0.9371 | 0.9506 | 1.0013 | 0.9998 |
| short-large | 0.9390 | 0.9397 | 1.0076 | 1.0004 |
| stream | 0.9882 | 1.0000 | 1.0084 | 0.9987 |
| large | 1.0079 | 1.0000 | 1.0056 | 1.0000 |
| tool | 0.9851 | 1.0000 | 0.9961 | 1.0000 |
| cancel | 1.0312 | 0.8750 | 1.0098 | 1.0008 |

## All further changes (versus attribution checkpoint)

| Workload | Wall | CPU | Sampled RSS | Lifecycle writes |
|---|---:|---:|---:|---:|
| short-small | 0.9904 | 1.0065 | 1.0041 | 0.9993 |
| short-large | 0.9518 | 0.9909 | 1.0030 | 0.9998 |
| stream | 0.9998 | 0.9500 | 1.0083 | 0.9994 |
| large | 1.0414 | 1.0455 | 0.9925 | 0.9997 |
| tool | 1.0088 | 1.0000 | 1.0098 | 0.9991 |
| cancel | 0.9582 | 1.0000 | 1.0114 | 1.0008 |

## Complete issue (versus original cloud production baseline)

| Workload | Wall | CPU | Sampled RSS | Lifecycle writes |
|---|---:|---:|---:|---:|
| short-small | 0.9613 | 0.9906 | 1.0089 | 0.9992 |
| short-large | 0.9245 | 0.9256 | 1.0073 | 0.9954 |
| stream | 1.0089 | 1.1111 | 1.0041 | 1.0025 |
| large | 0.9870 | 1.0435 | 1.0168 | 1.0000 |
| tool | 0.8441 | 0.8750 | 1.0085 | 0.9991 |
| cancel | 0.9248 | 1.0000 | 1.0106 | 1.0000 |

The compact Source stage improves the two repeated-turn CPU measurements by
about 5–6%, but total sampled RSS does not improve. At after-turns, paired median
RSS/anonymous PSS/file PSS differences in KiB are +508/−504/+1011 for short-small
and +2828/+1304/+1484 for short-large (component medians need not add).
The broader further-change comparison is mixed; the isolated Source gain must
not be advertised as the gain from keeping every candidate. Earlier reservation
and SQL stage adverse results, including the original six-pair canonical wall
regression, remain applicable historical evidence in their separate reports.

## Marginal canonical append

Thirty-two appends per fresh process, with no request/context construction.
Before is the preserved Source-only/schema-69 executable; after is the final
schema-70 implementation. Full ordered history is checked after reopening.

| Retained messages | Wall ratio | CPU ratio | Append process-write ratio |
|---|---:|---:|---:|
| 0 | 1.0277 | 1.5000 | 1.0000 |
| 64 | 0.9772 | 1.0000 | 0.9971 |
| 512 | 0.8913 | 1.0000 | 0.8836 |
| 2048 | 0.7232 | 0.7143 | 0.6397 |

The empty-history CPU result is adverse and coarse, not discarded. The existing
bounded integrity scan remains O(n); no whole-request O(1) claim or validation
removal follows from this matrix. Larger histories show the benefit of removing
history-wide JSON rewrite/allocation; context construction is measured separately.

## Cancellation investigation

The original reservation matrix had a 1.2926 wall ratio. Its slowest after sample
spent 150.353 ms inside submission (before 2.718 ms), with total elapsed 205.486 ms
and 80 ms aggregate CPU. Another pair was also slower; the submission outlier
alone does not explain all adverse observations. Existing endpoints cannot name
the underlying stall. First-consumed-publication time is observed after the
cancel/disconnect/release sequence, not a native publication timestamp.

Twelve additional balanced pairs use the exact original preserved executables,
continuing pair IDs 3–14. Their median wall/CPU ratios are 1.0130/1.0000; all
15 pairs together are 1.0344/1.0000. The original 1.2926 result is retained.
A stable 29% penalty was not reproduced, but the smaller adverse wall result
must not be declared zero or assigned a cause without evidence. Full per-pair
submission/settlement/CPU inputs and hashes are in the cancellation diagnostic.

## Fresh Pi controls

These are separate product/driver lifecycle measurements, not a claim that the
engines implement equivalent persistence, publication, recovery or tool contracts.
No old faux-provider or desktop ratio is reused. Each value is a median within
its engine, provided as absolute costs rather than a speedup claim.

| Workload/engine | Turn wall ms | Turn CPU ms | Lifecycle writes MiB |
|---|---:|---:|---:|
| short-small/rust | 66.086 | 95.000 | 127.883 |
| short-small/pi | 4.947 | 9.375 | 8.875 |
| short-large/rust | 73.017 | 105.938 | 329.551 |
| short-large/pi | 4.617 | 7.812 | 20.781 |
| stream/rust | 1585.171 | 200.000 | 6.234 |
| stream/pi | 1655.854 | 480.000 | 0.844 |
| large/rust | 216.539 | 230.000 | 13.707 |
| large/pi | 118.280 | 150.000 | 0.672 |
| tool/rust | 108.431 | 140.000 | 8.691 |
| tool/pi | 132.577 | 160.000 | 0.938 |
| cancel/rust | 48.346 | 70.000 | 4.707 |
| cancel/pi | 123.516 | 150.000 | 0.621 |

## Validation and reproduction

The exact committed production implementation passed 22 focused Source tests,
13 observation tests, 366 durable tests with scale modules excluded, and
all-target/all-feature Clippy with warnings denied. The SQL-only stage previously
passed all 370 durable tests including scales. Benchmark Python tests: 26 passed;
Pi oracle tests: eight passed. Tests include renamed/missing executable mappings
for heap analysis, negative request oracles, metadata corruption and reentrant
Source reads. No assertions, deadlines, recovery guarantees or lint policies
were relaxed. Earlier failed fixture/lint and superseded compile logs remain.

Commands and immutable executable identities are in the archived matrix status,
build logs and manifests. `benchmarks/runtime/README.md` documents each runner.
The external `run-continued-matrices.py` script records the exact six sequential
commands. All original evidence remains under `/workspace/issue-481-evidence`.
Allocation/CPU/write profiles are separate diagnostics; see the accompanying
profile report before deciding whether to retain the reservation and SQL stages.
Final exact-head FULL and independent review status must be recorded separately.
