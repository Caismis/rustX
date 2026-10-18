# Final diagnostic attribution, outside formal timing

All 18 fresh diagnostic runs passed: ordinary controls and 250 Hz CPU sampling
for short-small/short-large, one short-small syscall trace and one external
Source-temp footprint sampler for each of actual base, certificate and compact
production. These runs did not overlap formal matrices. Original binaries and
all raw profiles/logs remain preserved; no allocator was substituted. Native
allocation counts remain unavailable.

## Source carriers and index work

| Observation | Actual base | Certificate | Compact final |
|---|---:|---:|---:|
| Source byte files written | 442 | 442 | 442 |
| Byte files at most 4 KiB | 402 | 402 | 402 |
| Source index files written | 442 | 40 | 40 |
| Successful index `openat` calls | 1,207 | 160 | 160 |
| Index `pread64` calls | 4,948 | 1,000 | 1,000 |
| Index `pwrite64` calls | 2,892 | 480 | 480 |
| Index successful write bytes | 11,845,632 | 1,966,080 | 1,966,080 |
| Source-byte `pread64` calls | 91,712 | 40,753 | 40,753 |
| Database successful write bytes | 29,290,496 | 29,102,080 | 27,127,808 |
| WAL successful write bytes | 104,760,072 | 104,508,752 | 102,044,960 |
| Unresolved trace calls | 0 | 0 | 0 |

Source index opens approximate connection work; they are not exact connection API
or SQL statement counts. Successful syscall bytes are logical traffic, not
physical-device writes. Source temporary files are on tmpfs, so their reduced
index traffic cannot be presented as a disk-device write reduction. Database/WAL
categories include native runtime and the driver's verification/close/reopen work.
The trace extends through product exit; uncategorized output includes harness
evidence files and is not attributed to runtime persistence.

Bounded resident navigation removed small Source indexes; metadata reuse removed
repeated Source-byte reading. Compact pages do not further change those carrier
counts. Their measured persisted encoding/write reductions are separately visible
in the database/WAL observations and formal process counters. Small-fact tests
prove no temporary SQL spool table on the bounded memory path; exact whole-run
SQL statement counts are unavailable.

The 10 ms external sampler observed maximum simultaneous Source-temp logical /
allocated bytes of 53,855 / 57,344 at base and 53,001 / 53,248 at both later stages.
It observes product-owned `/tmp` `bytes` and `index.sqlite` paths discovered through
file descriptors. It can miss short-lived files and other temporary-file classes;
these are observed maxima, not physical memory peaks or a complete temp-file census.
Full per-path logical/allocated observations are archived.

## CPU samples and measurement work

The [plain counts](evidence/final-structural-cpu-counts.json) expose every reported
symbol numerator and an independently checked denominator from the complete raw
profile. Each steady profile covers 32 measured turns. Cumulative rows overlap;
never add them or treat overlapping elapsed scopes as CPU percentages.

| Stage | Case | Total samples | `sqlite3Prepare` cumulative samples | Counter sampler cumulative samples |
|---|---|---:|---:|---:|
| base | short-small | 744 | 158 | 18 |
| base | short-large | 845 | 188 | 21 |
| certificate | short-small | 716 | 136 | 23 |
| certificate | short-large | 786 | 153 | 24 |
| compact | short-small | 655 | 154 | 19 |
| compact | short-large | 761 | 166 | 28 |

SQLite preparation remains a substantial consumer. Its observed cumulative count
increases from certificate to compact in these individual profiles, so this is
not evidence that all SQL preparation was eliminated or generally reduced.
`process_sample` counts expose included driver counter work; they are not a
complete exclusive estimate of all harness overhead. Symbolization is optimized
native code, without a claim that every inline call has independent attribution.

## Diagnostic overhead and complete lifecycle

Each control/CPU row below is one diagnostic run, not a three-pair headline or
an exact causal overhead estimate. Signal/boundary handshakes and profile file
output are present only in CPU mode. Negative differences and variation are
retained; none are subtracted from formal measurements.

| Stage | Case | Control lifecycle ms | CPU-profile lifecycle ms | Control CPU ms | Profile CPU ms |
|---|---|---:|---:|---:|---:|
| base | short-small | 4191.250 | 4064.900 | 5230 | 5040 |
| base | short-large | 12183.187 | 12895.699 | 14830 | 15640 |
| certificate | short-small | 3917.778 | 3865.494 | 4920 | 4890 |
| certificate | short-large | 11747.445 | 12129.963 | 14310 | 14870 |
| compact | short-small | 3667.263 | 3731.148 | 4620 | 4750 |
| compact | short-large | 11410.352 | 11596.146 | 13980 | 14210 |

These lifecycle deltas run from before initialization through cleanup; absolute
self CPU including earlier startup is separately exposed in the formal
[lifecycle details](evidence/final-lifecycle-details.json). Formal phase deltas
retain verification, checkpoint/close, reopen, cleanup, provider startup/control
and controller completion. Evidence serialization/output follows the final
product counters and is included in complete controller wall instead.

Reproduce with `profiling/run.py`, `profiling/syscalls.py`, `profile-source.py`
and `profile-footprints.py` as recorded in the archive's command/status files.
The [compact archive](evidence/final-structural.tar.xz) and
[per-file manifest](evidence/final-structural.tar.manifest.json) retain full raw
profiles, trace joins, controls, sample outcomes, footprints and analysis scripts.
