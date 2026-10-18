# Continued attribution and candidate selection

These diagnostics describe the preserved `99da5d68` candidate and its preceding
SQL stage. They informed removal of the unproven SQL patch in `218b5043`.
Subsequent selected-combination measurements must identify their own executable.

## Allocation traffic

Same pinned gperftools 2.16, product-only preload and explicit phase boundaries
as `attribution.md`. Each row is one separate 32-turn diagnostic, with passing
full request/result oracles. Heap mode replaces the allocator and adds very
large overhead; these are neither native allocator counts nor headline timings.

| Stage | Requested allocations | Requested bytes | SQLite malloc/realloc bytes |
|---|---:|---:|---:|
| Attribution checkpoint | 8,146,523 | 1,687,264,799 | 631,200,520 |
| Reservations + SQL reuse | 7,994,748 | 1,590,776,800 | 631,740,776 |
| Plus compact Source | 7,906,444 | 1,518,976,183 | 583,880,208 |

Innermost named-owner allocation bytes for observation drain fall from
80,411,298 to 3,290,105 and then 3,260,839. Small Source index initialization
falls from 31,917,440 to 2,940,480 with compact metadata; original-byte copying
falls from 16,788,736 to 113,320 through right-sized buffers. These selected
owners are diagnostic attribution, not an additive decomposition of all costs.
Generic identical-code folding and absent inline expansion limit fine ownership.
The raw cumulative headers independently establish the total requested bytes.

The reservation change has a concrete allocation benefit and bounded extra
scalar work, despite no broad latency gain. Its cancellation confirmation and
residual adverse wall results remain in `continued-results.md`. Compact Source
has both an isolated repeated-turn CPU benefit and reduced allocation traffic;
it does not establish lower total sampled RSS. The SQL patch has neither a broad
paired runtime gain nor lower aggregate SQLite allocator traffic. Its selected
fragment/index owner rows also do not establish a reduction. It was removed;
we do not retain it merely because prepared-statement reuse sounds plausible.

## CPU samples and overhead

[Plain CPU counts](evidence/continued-cpu-counts.json) expose raw/report hashes,
executable identity and independently parsed denominators. CPU paths are relative
to the `continued-profile/` directory inside the profile archive. SQLite preparation
is 161/680 short-small samples and 151/732 short-large samples. Cumulative rows
overlap and must not be added. This still identifies substantial SQLite
preparation work but does not justify removing integrity checks or inventing a
cache authority. Marginal append retains its bounded O(n) validation scan.

Profiles include product threads and the driver. Counter/footprint helpers are
visible in the tables; some driver relay frames invoke production transport, so
there is no complete exclusive driver/product CPU split. Sampling requests
250 Hz. Synchronous profiler start/stop, dump and socket control costs belong to
diagnostics only. Heap SQL-stage measured wall averaged 5,128.47 ms/turn versus
roughly 70 ms in its unprofiled matrix: instrumented timing is unusable as a
production result. Each profile's complete lifecycle and control timings remain
in its raw sample; profiles did not overlap formal samples or competing builds.

The heap analyzer's explicit recorded executable path and renamed/missing-map
negative tests remain enabled. Symbol tables and first-load ELF headers are
retained alongside the profiles; no hard-coded executable basename is used.

## Logical write attribution

Separate sibling `strace`, attached before provider readiness. These are whole
40-turn lifecycle writes including live/reopened verification and cleanup;
evidence JSON is identified separately after the runtime cleanup endpoint.

| Destination | Successful syscall bytes |
|---|---:|
| Conversation WAL | 104,549,920 |
| Conversation database | 28,733,440 |
| SQLite temp files on tmpfs | 26,490,100 |
| Read-layout original byte files | 692,655 |
| Read-layout disk indexes | 1,966,080 |
| Evidence JSON after cleanup | 6,686,425 |
| Other | 57,796 |

The parser reconstructs every unfinished syscall (zero unresolved); the refined
category total equals the original parser total. There are 442 read-layout byte
files, 402 at most 4 KiB, and 40 disk-index files. The layout is shared with
canonical serialization; file categories alone are not exclusive ownership
attribution. Compact small Source construction performs no SQLite connection,
DDL, node inserts or index-sealing SQL. Larger/canonical disk paths remain.
Whole-runtime SQL statement counts were not instrumented; syscall opens must
not be presented as statement counts. Temporary tmpfs traffic and syscall-byte
counts are not physical-device writes. The original byte-file count is unchanged.

Verified profile archives retain raw CPU files, selected complete heap boundaries,
symbolized analyses and syscall logs. All original heap boundary files remain in
`/workspace/issue-481-evidence`; none were deleted. Runtime evidence and all
adverse candidate measurements are archived separately.
