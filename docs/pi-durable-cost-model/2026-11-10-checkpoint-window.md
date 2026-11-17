# Four-MiB checkpoint window: reject the measured production candidate

## Decision and source identity

**Reject this production candidate; retain the diagnostic fixture changes.**
The larger window consistently reduces writes, including through final close.
However, the actual candidate has adverse long-history ordinary timing/CPU,
more resident WAL, and no established memory saving. A same-ELF intervention
does not reproduce a consistent CPU penalty, so do not claim the policy itself
has been proved to cause the observed regression. Do not select the favorable
diagnostic timing in place of the ordinary candidate result, or tune more
thresholds in this iteration.

Baseline production: `1920e5141523a3e921d7c32f4e61889437e745e2`, Rust tree
`ed64a1b1f99937f6bc45f1c923d98247cc366fbf`. Measured candidate:
`414e71a65be1999b53b47db3a520dfb61de37758`, tree
`d5b7c11b017048ee913cd9735a4b3b1c112369b4`. Rejection:
`991e1f4fe61ff9f5f833d96c7e262072120e4ee1`. The restored `src`, `Cargo.toml`
and `Cargo.lock` diff against the baseline is empty. Later fixture/report
commits do not change that production tree.

| Ordinary executable SHA-256 | Baseline | Candidate |
|---|---|---|
| Conformance | `a682bdcf9cff9c049b150565ae0c83c701993d1f2f7bea920e5cb0702dbfaa80` | `b697b65a638832c43bf40d9286f81cca5af73a43e7d8e74a99a1f17395096f77` |
| Native App Server | `5c27b2671920f687ed520243e285b38fb3f7a15807ddc006905d59ced8f0b699` | `a1074f2ff1bd96c22d72b9d75b1367d04c90909c91a6b28d84ec6f81bba684f9` |

The candidate was freshly built with rustc 1.98.1, default features, ordinary
release O3/debug0 and eight build jobs. Root library, CLI and conformance
artifacts were fresh and their source paths checked. Baseline products reuse
the preceding verified build. Pi Durable `1cedd327` and pi-rust `4056176b`
remain source references; neither reference nor DeepSeek was run.

## The bounded experiment

The only production change explicitly sets `wal_autocheckpoint=2000` instead
of using the 1,000-frame default. Bundled SQLite 3.53.2, 2 KiB pages, WAL/NORMAL,
cache capacities, schema 105, protocols 76/85, eight-KiB streaming segments and
semantic commits remain unchanged. There is no delayed settlement, weakened
synchronous mode, new background checkpoint owner or on-disk representation.

Pi's Node adapter explicitly sets a 1,000-page threshold, but the source does
not set its page size. Compare byte windows rather than assuming equal page
counts are equivalent. pi-rust's append/progress design demonstrates separate
transient ownership, not a drop-in alternative to rustX's old cuts and bounded
independent reads. The existing write ledger already separates WAL traffic
from main-file checkpoint output; this experiment follows that attribution.

[SQLite's checkpoint documentation](https://www.sqlite.org/wal.html#checkpointing)
explains why changing the window can combine repeated main-page versions.
The threshold is not a hard WAL bound: an old reader can prevent checkpoint
completion. The candidate's deterministic test holds an old real-store
snapshot, writes beyond 2,000 frames while that reader continues to see its
original rows, then releases it, truncates WAL, closes and reopens the exact
final native head. The effective connection test confirms all policy values.

## Physical write result

One structural product per history/build, 32 hot turns, balanced order across
histories. An external strace and the existing phase seam supply the syscall
stream. WAL checksums/epochs, page owners at commit images, checkpoint page
versions and final reconstructed database are authenticated. These are
structural samples, not ordinary timing measurements or device writes.

| Setup turns | WAL KiB/turn before -> after | Main KiB/turn before -> after | Combined KiB/turn before -> after | Main syncs before -> after | Process writes through close MiB before -> after |
|---|---:|---:|---:|---:|---:|
| 8 | 450.16 -> 450.28 | 109.44 -> 93.19 | 559.60 -> 543.47 | 7 -> 4 | 23.793 -> 22.227 |
| 32 | 483.17 -> 482.78 | 140.44 -> 98.62 | 623.60 -> 581.41 | 8 -> 4 | 39.891 -> 36.625 |
| 64 | 500.30 -> 498.53 | 133.50 -> 79.06 | 633.80 -> 577.59 | 7 -> 3 | 62.156 -> 56.270 |
| 96 | 533.31 -> 535.14 | 188.12 -> 114.25 | 721.44 -> 649.39 | 9 -> 4 | 87.293 -> 78.613 |

Every product retains 160 hot write commits, 256 native events, 32 request
catalog bindings and 224 Trace definition rows. New native/Source node counts
match at each history: 285/319/351/352. No historical subtree rewrite or new
index removal occurred. WAL output is approximately unchanged; the savings
come from fewer main-file rewrites. Process writes through close also fall, so
the result is not solely displaced work past the hot endpoint.

Observed 8-to-64 combined growth falls from 74.2 to 34.1 KiB/turn. This is one
checkpoint-sensitive window at each depth, not a complexity proof or an
explanation of the older 131.1 KiB process-counter difference. Existing WAL
growth from index/frontier/manifest maintenance remains. See the full
[baseline](evidence/checkpoint-window-20261110/baseline-write-ledger.json) and
[candidate](evidence/checkpoint-window-20261110/candidate-write-ledger.json)
ledgers. Retained layout, syscall bytes and `/proc` output are distinct units.

## Ordinary result and fixed uncertainty check

Four balanced independent pairs cover the original four scenarios. Another
16 ordinary Tool/cancel products are supplementary functional checks. All 48
ordinary products pass; the original four have 552 complete matched semantic
request comparisons. Timing products have no tracer or preload, and the
whole-window sampled monitor observes no compiler. That is not proof of
exclusive machine use. Affinity, fixtures, build identities and every sample
are retained.

| Original cell | P50 ms, side medians before -> after | Paired P50 / P95 | CPU ms/turn, side medians | Paired CPU | Hot process KiB/turn | Paired writes |
|---|---:|---:|---:|---:|---:|---:|
| 8 setup turns | 9.768 -> 7.585 | -18.80% / -6.84% | 13.281 -> 11.406 | -15.69% | 594.062 -> 560.250 | -5.67% |
| 64 setup turns, initial | 7.993 -> 10.061 | +22.13% / -1.33% | 11.250 -> 13.750 | +14.30% | 697.000 -> 642.938 | -7.74% |
| 32 KiB burst | 30.503 -> 31.999 | +2.50% / +2.50% | 30 -> 30 | 0% | 592 -> 592 | 0% |
| Paced SSE | 1554.936 -> 1543.216 | -0.68% / -0.68% | 120 -> 120 | +4.17% | 788 -> 788 | 0% |

Percentages are medians of within-pair changes, not ratios of side medians.
CPU includes the conformance driver and has 10 ms endpoint granularity. Burst
and SSE each contain one turn per product. The [ordinary per-run CSV](evidence/checkpoint-window-20261110/ordinary-per-run.csv)
and [summary](evidence/checkpoint-window-20261110/ordinary-summary.json)
contain P50/P95 ranges, first consumed content, PSS, final-close writes and
retained size, including all adverse observations. None of these four-sample
cells establishes significance. The readable CSV uses LF; the archive retains
the original CRLF export with identical parsed fields.

All initial long-history P50 pairs increase. Before extending, the chat fixed
four additional long-history pairs to investigate this direction. The same
binaries, affinity, live subscriber and input are used. The extension has no
complete host monitor and does not replace the initial group.

| Long-history group | Paired P50 | Paired CPU | Paired hot writes |
|---|---:|---:|---:|
| Initial four pairs | +22.13% | +14.30% | -7.74% |
| Fixed four-pair extension | +20.32% | +14.51% | -7.76% |
| All eight pairs | +20.32% | +14.51% | -7.74% |

Seven of eight P50 pairs increase; ranges remain wide. Combined P95 change is
+4.95%. Through-close process writes fall about 7.57%, but final writer
open/close duration has a paired +18.20% change. This duration includes
validation and is not a pure checkpoint timer. Sampled per-session WAL maximum
rises about 2.129 -> 4.096 MiB. Read syscall counts fall approximately 14.17%,
so additional read-call count does not explain the higher CPU. No causal
binary-layout, scheduler or SQLite-internal explanation is established.
The [complete uncertainty audit](evidence/checkpoint-window-20261110/long-uncertainty-summary.json)
preserves both groups and their combination.

## Same-ELF causal diagnostic

To separate a policy intervention from two build identities, four balanced
64-history pairs use only the unchanged baseline ELF. A startup-only native
SQLite auto-extension installs a first-statement callback. That callback sets
1,000 or 2,000 frames, reads the effective value, and unregisters itself before
the workload. SQLite applies its normal default after auto-extension loading,
which is why setting the policy inside the extension alone would not work.
The provider does not inherit the preload. All nine diagnostic products,
including one smoke product, pass the normal native/request/reopen oracles;
all five connection policy samples per product match the selected value.

The [intervention identity](evidence/checkpoint-window-20261110/policy-causal-identity.json)
binds the baseline ELF, helper, header and executable-segment-checked native
symbol offsets. There is no continuing statement hook. These are diagnostic
products and must not be substituted for ordinary candidate measurements.

Paired diagnostic medians: P50 -5.29%, CPU -5.63%, P95 +11.75%, process writes
-8.00%, read syscalls -14.25%. P50/CPU increase in one pair and fall in three.
This does not reproduce a necessary policy CPU penalty; it also does not repair
the adverse measured production candidate or prove an isolated speedup. The
[complete result](evidence/checkpoint-window-20261110/policy-causal-summary.json)
keeps every pair. Do not attribute the ordinary regression to code placement
without more direct evidence, or keep rerunning until a preferred answer wins.

## Native 128-session supplement

One supplementary pair each for fixed and rotating four concurrency uses
128 sessions x eight turns, then 256 active turns. Native server counters
exclude the driver/provider but include native-idle retirement reads. Each
product completes 1,283 provider requests plus browse, slow-consumer, cancel,
close/reopen and three reload/unload checks. All 512 consistent backups are
independently authenticated for hashes, integrity/FKs, exact bodies/order and
cancelled-tail absence. No compiler is observed in active windows.

| Pattern | GNU anonymous PSS MiB at 128x8 before -> after | Logical WAL MiB at 128x8 | Active CPU ms/turn | Active writes KiB/turn |
|---|---:|---:|---:|---:|
| Fixed four | 667.19 -> 664.05 | 266.59 -> 379.88 | 17.15 -> 14.57 | 784.48 -> 728.45 |
| Rotating four | 660.77 -> 666.49 | 266.59 -> 379.89 | 13.95 -> 13.63 | 530.20 -> 530.86 |

Two endpoint samples per side do not establish a memory saving or equivalence.
PSS directions differ. Running WAL residency grows by about 113 MiB across
these sessions. It is not cumulative output or GNU PSS. A rotating active
window need not cross each connection's checkpoint threshold, explaining why
write savings are workload-sensitive without claiming all writes disappeared.
The [native audit](evidence/checkpoint-window-20261110/native-summary.json)
retains P50/P95, reads, notifications, allocated/logical file residency and
compiler samples. All remaining WAL files are absent or zero at unload.

## Correctness, restoration and next action

The candidate passed all-target/all-feature Clippy with warnings denied,
formatting and 1,641 focused library tests, including the explicit
policy/slow-reader cases, with three ignored. The 48 controller tests pass. Those Rust checks
apply to the rejected candidate. Its new slow-reader test and policy change
were reverted together. The retained capacity fixture passes Python
compilation and its four complete native products; final formatting and all
48 controller tests also pass after restoration.

The current production tree exactly equals the already measured baseline,
whose ordinary, structural and native products above remain successful. No
new Rust optimization, checkpoint setting, schema gate, compatibility path or
failure contract is retained. Fixture-only changes record intermediate depths,
notification counts and per-phase main/WAL/SHM logical and allocated residency.
The source and build/disposition checks are in the archive.

The [separate soft-budget experiment](2026-11-10-process-cache-policy.md) was
also rejected. These outcomes redirect the active objective toward fewer
retained cut/metadata representations and fewer actual hot access structures,
rather than another quota or checkpoint value. A compile-only public-type
probe confirms shallow sizes of ContentSource/ContentRef/ContentBinding/
CollectionFrontier at 88/136/192/64 bytes; that is not a unique-owner census or
evidence that boxing them removes the observation allocation group. The failed
GDB type lookup is archived and is not a correctness gate.

**Reject the measured candidate; broader optimization remains active.** Do
not describe a shipped 5-8% write improvement. The physical result is useful
evidence, while its delivery tradeoffs remain unacceptable for this iteration.
Keep #485 Draft and do not merge. Exact-final-HEAD CI state is recorded after
push without waiting for CI; prior candidate checks are not final integration
readiness.

The [archive](https://drive.google.com/file/d/1h7XPTYks7HMenaDgDRizkVxotm4f3oWF/view?usp=drivesdk "Original file: docs/pi-durable-cost-model/evidence/checkpoint-window-20261110/evidence.tar.xz") and
[manifest](evidence/checkpoint-window-20261110/manifest.json) retain every
product, ordinary/diagnostic identities, full traces, authenticated replay,
native backups, negative results, source patch and checks. Product binaries,
dependencies, credentials and live roots are excluded. Helpers retain absolute
paths; no portable replay claim is made.

Detailed evidence links open the containing Drive bundle. See [download and checksum instructions](../performance-evidence/README.md) for original member paths and byte-level verification.
