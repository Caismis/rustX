# Compact lifecycle keys and current memory attribution

## Decision and identity

**Accept with notes for the two compact kind indexes.** The ordinary short/long write reductions are consistent across all four pairs, and physical ownership confirms fewer writes to the targeted B-trees. The candidate has adverse short-history timing/CPU and a modest 128-session memory increase; it is not an across-the-board performance improvement. The broader history-growth objective remains active. No further body-sharing production change is claimed here.

Production baseline: `43ab0356d9fcd4d5f6485fa73b9d15ab2618d67f`, source tree `75a6f89ac3faf591ae763664cc728f45dbd46e0c`. Candidate: `1920e5141523a3e921d7c32f4e61889437e745e2`, source tree `ed64a1b1f99937f6bc45f1c923d98247cc366fbf`. Both were freshly built with rustc 1.98.1, default features, ordinary release O3/debug0 and eight build jobs. The final fixture/report changes do not change this candidate's Rust tree.

| Executable | Baseline SHA-256 | Candidate SHA-256 |
|---|---|---|
| Conformance | `266f3aee03d201af1acfea66bc978aaa8e244cb31359d20ab7cbcf7d2bcba48d` | `a682bdcf9cff9c049b150565ae0c83c701993d1f2f7bea920e5cb0702dbfaa80` |
| Native App Server CLI | `4c4e329507dc64abaec1d550368d353c5379a997b9b0c02851d87c5bdb13f4b4` | `5c27b2671920f687ed520243e285b38fb3f7a15807ddc006905d59ced8f0b699` |

Pi Durable `1cedd327` and pi-rust `4056176b` remain read-only source references. No reference product or DeepSeek was rerun. The [preceding plan](2026-11-10-history-growth-plan.md) describes their lifecycle and immutable-owner lessons. This report supersedes its unretained kind-sketch status and revises its memory priority using current measurements.

## Retained representation

Keep `events_kind_idx` derived from native `event_json` and `events_primary_trace_kind_idx` derived independently from `observation_kind`. Eight common lifecycle strings become disjoint two-byte private BLOB keys. Other text and non-BLOB SQL types retain their typed values; arbitrary BLOBs acquire a separate prefix and cannot alias valid codes. Public event kinds, native facts and all scoped indexes remain unchanged.

The SQLite expression and Rust query parameters share one explicit mapping contract, with exhaustive closed-kind/type tests. Queries use the indexed expression directly, retain sequence bounds and ordering, and do not add a table scan, lookup cache, persisted column or application-defined SQLite function. Agent Trace filtering still admits only its three request kinds. Independent native and observation corruption remains observable.

Schema 105 explicitly rejects schema 102 and other old development layouts. The regression reconstructs the preceding two index definitions and checks both open APIs reject without changing file bytes. There is no migration or legacy reader. WAL/NORMAL, 2 KiB pages, 1,000-frame auto-checkpoint and final-close behavior are unchanged.

## Physical writes and history growth

One fresh instrumented product per history/build, each with 32 hot turns. WAL checksums, epochs and page ownership are authenticated at commit images; main-file versions are matched to WAL images. These samples establish structural work, not headline timings or statistical effects. Workspace paths differ between diagnostic directories, and their logical column totals remain in the evidence.

| Setup turns | WAL frames before → after | WAL + main KiB/hot turn before → after | Process writes through final writer close, MiB before → after |
|---|---:|---:|---:|
| 8 | 7,346 → 7,118 | 574.89 → 559.60 | 24.35 → 23.83 |
| 32 | 7,915 → 7,630 | 642.80 → 623.03 | 40.99 → 39.90 |
| 64 | 8,129 → 7,882 | 667.90 → 632.72 | 64.19 → 62.19 |
| 96 | 8,664 → 8,413 | 717.54 → 720.29 | 89.69 → 87.84 |

All eight products retain 160 write commits, 256 appended native events, 32 catalog bindings and 224 definition rows in the hot window. Native/Source new node counts are equal per history (285, 319, 351, 352). Existing subtree reuse is preserved. This change does not rewrite canonical facts or combine semantic commits.

The targeted indexes' WAL plus main-file traffic drops from approximately 33.5 to 17.8 KiB/turn at eight setup turns, and 52.4 to 30.4 at 64. At 96 turns, additional checkpoint main-file traffic outweighs the hot WAL saving. This adverse endpoint is retained. The earlier reused schema-102 reference also showed the 96-turn reversal; the fresh exact-build reference confirms its direction. Total process writes through close fall at all four histories, so work is not merely postponed beyond the hot endpoint. Syscall payloads, process write_bytes, retained dbstat pages and device writes remain distinct units.

## Ordinary fake-provider comparison

Four independent balanced/interleaved pairs in each of the original four cells, without tracing. All 32 products and 552 complete semantic request comparisons pass. CPU includes the in-process conformance driver. Single-turn CPU endpoints have 10 ms granularity. Per-run P50/P95 distributions and every negative sample remain in the [summary](evidence/kind-keys-20261110/kind-ordinary-summary.json) and [per-run CSV](evidence/kind-keys-20261110/kind-ordinary-per-run.csv). Percent changes below are medians of within-pair changes, not ratios of side medians.

| Case | P50 ms, side medians before → after | Paired P50 / P95 | CPU ms/turn, side medians before → after | Paired CPU | Hot write KiB/turn before → after | Paired writes |
|---|---:|---:|---:|---:|---:|---:|
| 8-turn history | 8.53 → 10.69 | +26.67% / +20.72% | 12.03 → 13.59 | +17.94% | 607.4 → 593.9 | -2.22% |
| 64-turn history | 13.07 → 8.15 | -36.90% / -29.66% | 16.88 → 11.25 | -29.46% | 744.2 → 695.9 | -6.28% |
| 32 KiB burst | 30.86 → 31.01 | +1.98% / +1.98% | 25.00 → 30.00 | +25.00% | 592.0 → 592.0 | +0.00% |
| 128-chunk paced SSE | 1555.18 → 1550.03 | -0.33% / -0.33% | 120.00 → 130.00 | +3.85% | 788.0 → 788.0 | +0.00% |

Short-history P50 and CPU rise in three of four pairs, while long-history P50 and CPU fall in all four. Short P50 changes range from −47.30% to +51.97%; the narrow source change does not establish a causal explanation for that spread. Do not claim the long cell proves a dependable 37% latency improvement or that the short regression is resolved. Burst paired P50 is +1.98%; paced SSE is −0.33%. Their hot writes are unchanged. Process writes through close fall by paired medians of 1.94% and 3.34% in the two short-answer cells. The observed difference between eight- and 64-history side medians shrinks from 136.9 to 102.0 KiB/turn; it is still substantial.

## Four-concurrency and 128-session checks

The capacity fixture now uses native protocol 76: base64url bearer subprotocol, thread/start and thread/resume, unique inputId, and turnId-bound interruption. Sessions receive explicit names through the normal API so auxiliary automatic titles do not consume the finite provider sequence. Naming is a fixture workload choice, not a production feature removal. A memory-only mode preserves native close/reload and exact database verification.

The full fixed/rotating fixture separately records terminal-delivery latency and native-idle retirement. The next input waits for current native idle rather than racing the previous attempt's closed steering gate. CPU windows include these status reads. This endpoint correction is necessary for current semantics and differs from the old v52 fixture. It also records fixed versus rotating active groups explicitly.

Two balanced pairs each for fixed and rotating four concurrency: eight native server products, 128 subscribed sessions × eight setup turns each, then 256 active turns. CPU and I/O count only the server PID, excluding sibling provider and driver. The [native audit](evidence/kind-keys-20261110/kind-native-audit.json) preserves individual products and ranges.

| Active group | CPU ms/turn before → after | Paired CPU | Hot write KiB/turn before → after | Paired writes |
|---|---:|---:|---:|---:|
| fixed | 14.609 → 14.766 | +1.07% | 807.01 → 789.21 | -2.21% |
| rotating | 13.867 → 13.652 | -1.53% | 534.38 → 530.67 | -0.69% |

The fixed CPU cost is about +1% in both pairs; rotating is −3.06% and 0%. This does not reproduce a universal large CPU regression and does not explain the ordinary short-history cell. Another worktree compiled during one baseline fixed active window (six half-second monitor samples); its result is retained, not silently discarded. No compiler was observed in the other seven active windows, but monitoring is sampled and cannot certify exclusive machine use. Two pairs are not a significance test.

Before active extra work, all eight native products have the same 128×8 endpoint. Ordinary GNU-allocator anonymous PSS:

| Endpoint | Baseline median MiB | Candidate median MiB | Median paired change |
|---|---:|---:|---:|
| 128 subscribed empty sessions | 337.75 | 339.71 | +0.63% |
| 128 sessions, eight turns each | 657.18 | 667.57 | +1.72% |

Three of four eight-turn pairs increase by 10.0–13.9 MiB; one decreases by 1.3 MiB. This is a write optimization with a modest adverse memory observation, not a memory saving. An earlier single memory-only pair showed 660.64→656.99 MiB; it is also retained and is not used to replace the larger paired result. Neither set is comparable to the old `e7289ac18` 480.75 MiB endpoint because source, protocol and fixture behavior changed.

Each full product completes 1,283 provider requests, including browse, slow consumer and cancellation. Independent verification reopens all 1,024 consistent SQLite backups, authenticates their hashes, checks integrity/FKs, exact user/Assistant content and ordering, committed counts, and absence of the cancelled provider tail. Each product also performs three reload/unload cycles. Tool/cancel conformance has an additional paired functional check; it is not pooled into performance samples.

## Where current retained memory grows

A separate diagnostic uses the unchanged baseline CLI with the existing tcmalloc sampler: complete 64 KiB Poisson exports at four explicitly acknowledged phases. Raw headers are re-summed and independently weighted totals match symbolized stacks. These requested-byte estimates and allocator class-charged totals do not partition ordinary GNU PSS. Priority groups classify allocation origins, not authoritative object owners. In particular, the small model bucket is not an upper bound on all canonical payloads.

| Requested allocation origin, estimated MiB | Subscribed empty 128 | 128×8 |
|---|---:|---:|
| SQLite | 91.82 | 280.79 |
| Checkpoint capture | 0.44 | 41.40 |
| Other observation | 39.26 | 110.51 |
| JSON Schema validation | 40.19 | 40.19 |
| Model-related remainder | 5.58 | 7.71 |
| Local-runtime remainder | 38.08 | 37.69 |
| Other | 14.72 | 22.88 |
| Total | 230.10 | 541.17 |

Within the SQLite stack group, page allocations are about 25.3→129.7 MiB, statement-compilation allocations 18.4→103.0, schema initialization 41.0→41.0, lookaside 5.6→5.6, and other SQLite about 1.6. These are mutually prioritized stack origins, not direct db_status measurements. Approximately 97% of the sampled empty-to-eight-turn requested-byte growth falls in SQLite and the two observation groups. It is stronger evidence for those directions than for a broad canonical-input API rewrite.

After unload, the diagnostic allocator's class-charged live bytes drop from 595.37 to 43.09 MiB, while its heap retains 672 MiB of spans/free capacity. Ordinary GNU products also retain many physical pages. Object release, allocator free space and OS page return must be tracked separately. Do not add another allocator policy or shorten cache TTL merely to improve this endpoint. Existing 30-second page and 120-second statement reclamation remain.

The next memory action is an owner census around freshly read transcript rows, response proof invalidation, prepared decoration and replay-held cuts. `PreparedTranscriptRow::with_decoration()` currently creates another prepared/native row even when newly validated decoration has the same exact representation. Determine its retained contribution and share exact immutable presentation only after fresh storage proof; preserve signed floating-point representation, failure isolation and old cuts. The heap also confirms per-registration canonical schema compilation contributes to the empty baseline; evaluate immutable schema-owner sharing separately if it is the largest actionable remaining fixed cost. Neither mechanism is implemented or counted as a saving here.

## Verification and disposition

Local checks for the retained Rust source: formatting and diff checks; all-target/all-feature Clippy with warnings denied; 1,533 focused all-feature library tests passed, three ignored. The effective-policy test confirms bundled SQLite 3.53.2, WAL/NORMAL, 2 KiB pages and 1,000-frame auto-checkpoint. All 48 benchmark-controller tests pass. Capacity scripts compile as Python and pass full small current-protocol scenarios, including interruption and native retirement. Ordinary, physical and capacity products keep their distinct verification purposes.

Preflight failures remain in the archive: initial bearer encoding, unexpected automatic-title requests, refusal to measure during another compiler, and the terminal-to-retirement race in the old fixture. No failed product is included as an accepted sample. The source change preserves exact frozen/retry input, canonical settlement, durable release, cancellation/start-failure behavior, terminal ordering, bounded queries, current-byte/cut checks, rollback and reopen as covered by the focused suites and product oracles. This is not a claim that every project integration gate ran locally.

**Accept with notes for compact keys; broader optimization and merge readiness are incomplete.** Keep #485 Draft. Exact-final-HEAD remote CI is recorded separately after push; do not wait for it or merge as part of this work. Remaining history growth is not eliminated, and no new memory improvement is claimed. Proceed to evidence-led SQLite/observation ownership work rather than another unrelated small allocation optimization.

The [evidence archive](evidence/kind-keys-20261110/evidence.tar.xz) and manifest preserve build identities, frozen fixtures/helpers, complete ordinary records, authenticated physical products, sampled heap exports/symbolization, full native products/consistent backups, failures and checks. Actual executables/dependency trees, mutable runtime roots and token files are excluded; their identities and native-backup hashes remain. These stored checks do not freshly verify excluded executable bytes on another machine.
