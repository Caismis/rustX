# Equal transcript presentation: rejected capacity candidate

## Decision and source identity

**Reject this production candidate and retain the benchmark corrections.** Sharing a prepared transcript row after fresh response validation is functionally sound in the tested cases, but it does not materially reduce ordinary 128-session memory, deep-history memory, or sampled live allocation. Four-concurrency results also contain adverse CPU observations. Do not expand this mechanism or present its short-history timing result as a shipped improvement.

Measured baseline: `1920e5141523a3e921d7c32f4e61889437e745e2`, Rust tree `ed64a1b1f99937f6bc45f1c923d98247cc366fbf`. The benchmark/report-only baseline `4f62aef13` has the same production source. Candidate: `91a3ffb422ba1096cb351e375139a75fd26310e2`, Rust tree `5a6e2558103b72cb67f54eb30b0f2a248bf3a620`. Revert: `535ace304cd6f9cdb2009ae96649028af6839933`. The restored `src`, `Cargo.toml`, and `Cargo.lock` diff against the measured baseline is empty. Final documentation does not change that Rust tree.

| Preserved ordinary executable | Baseline SHA-256 | Rejected candidate SHA-256 |
|---|---|---|
| Conformance | `a682bdcf9cff9c049b150565ae0c83c701993d1f2f7bea920e5cb0702dbfaa80` | `8aa91368de0bbb1bb913d1926c5edb64250e9fd81da0a6d5c7a68ad2eafc482f` |
| Native App Server CLI | `5c27b2671920f687ed520243e285b38fb3f7a15807ddc006905d59ced8f0b699` | `89e6b95e3b66b71a36bdf7573f878c025b655d7f86fb5de9059f65a12b208c67` |

The candidate was freshly built with rustc 1.98.1, default features, ordinary release O3/debug0 and eight build jobs; root library, conformance and CLI artifacts were rebuilt and their checkout paths recorded. Baseline products reuse the preceding verified build. Actual executables are excluded from the archive, with their hashes and build records preserved. SQLite schema 105, WAL/NORMAL, 2 KiB pages, 1,000-frame auto-checkpoint, semantic commit boundaries and public protocols 76/85 are unchanged.

Pi Durable `1cedd327` and pi-rust `4056176b` remain read-only source references. Neither reference nor DeepSeek was built or run. Their latest-only temporary presentation and authenticated cold-payload ownership inform the investigation; rustX's historical cuts and independent failures remain required. This is a rustX baseline/candidate decision, not a new cross-product comparison.

## What was tested and removed

The existing response cache already reuses complete rows when the authenticated response owner matches. The candidate additionally reused the immutable presentation after the reader obtained fresh current descriptor, Tool and response facts and found identical decoration. It retained current-byte validation, old cuts, changed-decoration allocation and release rules. The optional floating-point output rate used bit equality in addition to typed equality, preserving signed zero; NaN conservatively prevented reuse.

Deterministic tests demonstrated owner reuse through 63 forced fresh reads, release of the redundant read owner, unchanged admission/normalization work, oversized/deferred rows, signed-zero bytes and old-cut immutability. A real-store test invalidated response proof, required fresh native queries, verified shared presentation, injected corrupt current header bytes and required a read error, then verified rollback recovery. These tests prove the local mechanism; they do not establish its frequency or retained contribution in ordinary conversations.

The production change and its new tests were reverted together. The following fixture changes remain:

- `4f62aef13`: memory-only samples at specified idle times without extra turns or unloaded actors. Both subscriptions and all loaded runtimes remain present; manager diagnostics do not read conversation SQLite.
- `9277f636a`: concurrent `thread/read` calls respect the native two-slot metadata lane instead of sharing the six-slot ordinary semaphore.

The original depth baseline failed with `request_capacity`. Its log and partial product are retained. Source inspection confirmed the fixture lane mismatch, and both corrected 16x64 products passed. No semantic request is silently retried. The eight accepted immediate-memory products use frozen fixture v1; depth, strict idle, concurrency and heap products use corrected v2. Each before/after pair uses the same fixture. The earlier products are not retrospectively relabeled as v2.

## Ordinary memory and historical growth

Four balanced/interleaved 128x8 pairs use ordinary GNU allocation. Values below are side medians of sampled anonymous PSS, not diagnostic requested bytes or continuous peaks. The [memory audit](evidence/row-owner-20261110/memory-audit.json) retains every value and paired change.

| Endpoint | Baseline MiB | Candidate MiB |
|---|---:|---:|
| 128 subscribed empty sessions | 337.52 | 336.89 |
| 128 sessions, eight turns each | 662.44 | 662.13 |
| After unloading the 128 sessions | 665.38 | 664.96 |

The median of within-pair eight-turn changes is −0.20%, with two increases and two decreases; individual changes span approximately −1.04% to +0.58%. This differs from dividing side medians. Neither supports a material capacity improvement. Object release and physical page return remain different questions: the unload PSS does not by itself prove objects are still alive.

Supplementary products use one pair per condition, not four-sample estimates:

| Condition | Baseline anonymous PSS MiB | Candidate MiB |
|---|---:|---:|
| 16 sessions, 64 turns each | 214.88 | 216.35 |
| Strict unchanged 128x8, immediate | 660.48 | 644.24 |
| Same 128x8, 32 seconds idle | 600.93 | 597.46 |
| Same 128x8, 130 seconds idle | 557.71 | 555.29 |

The isolated immediate idle-pair decrease does not replace the four-pair result. Its difference contracts to 2.42 MiB after existing cache reclamation. Deep history rises by about 0.68%. No extra active turns, reconnects or Inspector browsing occur between the strict idle endpoints.

Every accepted native product retains close/reopen checks and three reload/unload cycles. Independent audit reopens 2,592 consistent backup databases across the 12 memory, eight concurrency and two heap products; verifies each backup hash, integrity, foreign keys and schema; and checks exact canonical content, order and counts. All 24,600 finite provider requests in these accepted products passed their fixture oracles. The failed original depth product is excluded from these accepted counts and retained separately.

## Allocation evidence changes the next action

One separate diagnostic product per build uses the existing 64 KiB Poisson sampler and replacement tcmalloc, with six acknowledged phases. All raw headers are re-summed; independent corrected totals match symbolized integer weights within rounding. The [heap audit](evidence/row-owner-20261110/heap-audit.json) preserves complete raw/symbolized evidence and allocator properties. These allocation origins are not a partition of ordinary GNU PSS or a unique-object census.

| Diagnostic unit, 128x8 | Baseline MiB | Candidate MiB |
|---|---:|---:|
| Allocator class-charged live bytes, immediate | 596.36 | 596.23 |
| Same unit, 130 seconds idle | 348.42 | 348.91 |
| Same unit, after unloading | 41.93 | 42.25 |
| Sampled requested estimate, immediate | 540.65 | 537.41 |
| Sampled requested estimate, 130 seconds idle | 314.94 | 313.81 |
| Prepared-transcript inclusive stack presence, immediate | 9.49 | 9.80 |

The prepared-transcript presence counter overlaps other groups and is not an upper bound on semantic row ownership. It provides no evidence of a large removed allocation group. Class-charged live memory is essentially unchanged, supporting rejection of this capacity hypothesis.

The baseline's matched diagnostic gives a more useful retained-cost map:

| Priority allocation origin | Immediate MiB | 130 seconds idle MiB |
|---|---:|---:|
| SQLite | 255.17 | 58.98 |
| Other observation | 127.34 | 117.35 |
| Checkpoint capture | 39.40 | 39.40 |
| Schema validation | 39.89 | 39.89 |

Within SQLite, page-origin requested bytes fall from 127.93 to 10.62 MiB, and statement-compilation origins from 79.58 to 0.75 MiB. Schema initialization remains about 40.03 MiB and lookaside about 6.17 MiB. Thus existing idle policy is releasing substantial live cache allocation; it is inaccurate to call the immediate 255 MiB permanently retained SQLite payload. It is also inaccurate to derive GNU allocator free space by subtracting these replacement-allocator units from GNU PSS.

After reclamation, observation/checkpoint origins still total about 156.75 MiB. That result favors investigating the retained per-publication/cut metadata and its actual owners, alongside the SQL/page working set, rather than extending equal-row decoration comparisons. Generic model-allocation buckets do not bound all canonical bodies. Stable schema compilation remains a separately confirmed fixed-cost candidate, not an explanation of all history-dependent growth.

## Ordinary performance and adverse cases

Four independent balanced/interleaved pairs per original scenario, 32 ordinary products and 552 complete before/after semantic request comparisons, all pass. Each run's P50/P95, ranges, CPU, first consumed content, process writes, final close and retained growth remain in the [summary](evidence/row-owner-20261110/ordinary-summary.json) and [CSV](evidence/row-owner-20261110/ordinary-per-run.csv). Do not pool turns into independent product samples.

| Scenario | P50 ms, side medians before → after | Paired P50 | Paired CPU | Paired hot process writes |
|---|---:|---:|---:|---:|
| Eight-turn setup | 11.74 → 7.29 | −23.02% | −19.26% | +0.13% |
| 64-turn setup | 10.02 → 9.38 | +2.48% | −5.88% | −0.04% |
| 32 KiB burst | 34.23 → 32.85 | −1.83% | 0% | 0% |
| Paced SSE | 1554.84 → 1553.34 | −0.10% | −7.69% | 0% |

Long-history paired P50 changes span −47.69% to +121.01%; its worst P95 change is +991.06%. SSE first-consumed time has a paired median +3.14%. All adverse results are retained. No compiler was observed by the ordinary-run host monitor, but sampling does not establish exclusive machine use or explain the large variation. No dependable latency percentage is attributed to this narrow source change. CPU endpoints have 10 ms granularity. The read-only change does not remove persisted facts, tables, indexes or transactions; no physical-write improvement is claimed.

Two pairs each of fixed/rotating four concurrency use 128x8 setup followed by 256 active turns. Only server PID CPU/I/O is counted; CPU includes native-idle retirement status reads. The [native audit](evidence/row-owner-20261110/native-audit.json) retains every product and active-window overlap.

| Group | Individual paired CPU changes | Median paired CPU | Median paired hot writes |
|---|---|---:|---:|
| Fixed four | +25.42%, +7.24% | +16.33% | −0.06% |
| Rotating four | +22.85%, +16.86% | +19.85% | approximately 0% |

The first candidate active window in each group overlaps an external compiler: 16 and 20 half-second monitor samples. These samples are preserved, not silently discarded. The other pair in each group has no observed compiler overlap and still has higher CPU. This supports rejecting the candidate without claiming the measured percentages are isolated causal penalties. Do not repeat products merely to obtain a favorable result. The final retained Rust tree is the baseline tree; these candidate penalties are not shipped.

## Correctness and disposition

The candidate passed all-target/all-feature Clippy with warnings denied, formatting/diff checks, 15 focused prepared-transcript cases and the real-store revalidation test. The broad targeted suite recorded 1,642 passes and one child-process failure due to a stale schema-103 debug CLI. After rebuilding the CLI/supervisors, all nine child-delivery tests passed, including the failed case. The initial failure and refreshed prerequisite evidence remain archived. These are candidate-source checks, not a claim of complete project integration validation.

Retained fixture checks: Python compilation, all 48 runtime-controller tests, formatting/diff checks and the full accepted ordinary/native products above. Native baseline products exercise the exact restored production source. No new storage format or compatibility path is introduced. Cancellation/start failure, frozen/retry identity, atomic canonical ordering, terminal-last, old cuts, bounded reads, resync, rollback/reopen and independent-domain boundaries are not intentionally changed.

**Reject the presentation-reuse production change; accept the fixture corrections. Broader optimization remains active.** The current evidence identifies SQLite's active working set and retained observation/cut ownership as substantial history-growth workstreams. It does not justify claiming those costs eliminated, deleting historical query indexes, restoring sparse Statistics replay or shortening replay to manufacture a result.

The [archive](evidence/row-owner-20261110/evidence.tar.xz) and [manifest](evidence/row-owner-20261110/manifest.json) preserve accepted and failed products, exact builds, fixture revisions, candidate patch, checks, backups and complete diagnostic profiles. Helpers retain historical absolute paths; this is inspectable evidence, not a claimed portable replay bundle. Keep #485 Draft. Exact-final-HEAD remote CI is reported separately after push; do not wait for it or merge as part of this work.
