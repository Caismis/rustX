# Fresh cloud baseline and Source profile

Production base: `3c355f152d405a201482d53b6e45497df4f44a7f`.
Harness: `8824728b951150f5b77e6431d97b90ef8938ae47`.
Optimized executable SHA-256:
`287806e2c3515cdd2d9a05842f17ab31497205a907f8327febde1e08b3a59e0b`.
SQLite schema 69; App Server protocol 50; default Cargo features, release profile.
Pi Durable: `90d863f7c1183b965f099855715d6e88c7b8a52c`, Node 24.19.0.
The saved cloud runner exposes five AMD EPYC 9V74 vCPUs and Linux 6.18.55.
Build and hardware details are in `baseline-identity.json` in the archive.

Three paired interleaved repeats per case passed: 36 samples, 18 paired core
comparisons, 846 requests. Both production adapters called the same local
HTTP/SSE script. Each process/store was fresh; OS cache was warm, not dropped.
No build or other benchmark overlapped these formal samples. The prior desktop
results and in-process Pi faux-provider results are not comparable baselines.

The table is the median of three per-run values; every raw run, including
failed smoke attempts, is preserved. Each short-case turn value is that run's
mean of 32 measured turns after separately accounted seeding.

| Case | rustX turn ms | Pi turn ms | rustX CPU ms/turn | Pi CPU ms/turn | rustX lifecycle writes MiB | Pi lifecycle writes MiB |
|---|---:|---:|---:|---:|---:|---:|
| short-small | 70.93 | 4.63 | 100.94 | 9.06 | 128.02 | 8.88 |
| short-large | 76.27 | 4.50 | 106.25 | 7.50 | 330.76 | 20.78 |
| stream | 1591.60 | 1639.09 | 210.00 | 480.00 | 6.24 | 0.84 |
| large | 211.67 | 118.87 | 230.00 | 150.00 | 13.71 | 0.68 |
| tool | 116.83 | 128.45 | 140.00 | 150.00 | 8.70 | 0.94 |
| cancel | 51.79 | 122.30 | 70.00 | 160.00 | 4.71 | 0.62 |

CPU is self user+system `/proc/stat` accounting at 100 ticks/second (10 ms
resolution), not exclusive engine attribution. Runtime and driver work are
included. Product lifecycle includes initialization, seed/turns, verification,
shutdown, checkpoint/close, reopen verification and cleanup. Provider startup
wait is explicit in the lifecycle; provider CPU/reaping are excluded because
it is a sibling owned by the controller. Product evidence-file writing follows
the final cleanup counters and is included only in the separately reported
controller process wall. Per-phase deltas isolate verification costs without
subtracting unrelated medians. RSS/PSS are sampled, never claimed as true peaks.
Allocation profiling and exclusive CPU sampling are unavailable. Writes are
process-attributed OS accounting, not physical device writes. Native schema and
retained-contract differences remain explicit; this is not feature parity.

## Separate syscall profile

The successful strace run is outside formal timing (27.16 seconds under
instrumentation versus approximately 4 seconds without it). It includes the
product and its threads, not the sibling provider. Joined unfinished/resumed
records have zero unresolved calls. It found 442 Source byte files, 402 of
which held at most 4,096 bytes, plus 442 temporary index files. Source index
work was 1,207 opens, 2,892 writes, and 11,845,632 successful written bytes.
Source byte work was 692,643 written bytes and 91,712 positional reads.
These are syscall operations, not counts of SQL statements or CPU percentages.

Selected first candidate: retain the existing SQLite navigation index in memory
only for Sources whose completed exact encoding is at most 4 KiB. Bound its
SQLite pages, keep the original bytes and parser, and leave larger Sources on
the existing disk path. This removes small-source index files/reopens without
re-encoding, deferring indexing, changing witnesses, or changing durable formats.
Its effectiveness and memory consequences must be measured before acceptance.

## Reproduction and evidence

Commands: `benchmarks/runtime/controller.py`, `summarize-http.py`, and
`profile-source.py`, with the pinned build command in `progress.md`. The profile
command and each run's complete argv are retained in their process records.
The original raw evidence remains in `/workspace/issue-481-evidence`.
`evidence/baseline.tar.xz` is a compact copy with `MANIFEST.json`; the separate
manifest lists each original relative path, byte count, and SHA-256.

Five Rust and eight Node oracle regressions passed. The initial Rust smoke
records remain failed: the controller read an unflushed log and rejected the
otherwise passing product run. The controller was repaired before formal runs;
none of those failed samples is relabelled or included in the baseline table.

Cloud filesystem qualification: `/tmp` is tmpfs, while `/workspace` is an
overlay filesystem. The Source temporary files live on tmpfs in these runs;
the Source index syscall write bytes above are logical file traffic and cannot
be counted as physical disk writes or expected reductions in `/proc/io`
`write_bytes`. Runtime databases live under the evidence directory on the
workspace filesystem. This mount arrangement is held fixed across revisions.
