# Shared content-reference owners: retained growth improves, cold burst remains adverse

## Decision and identities

Retain the reference-owner mechanism provisionally for the combined candidate. It reduces actual retained allocations and all four ordinary 128-session memory pairs. Reject the extra shared-path wrapper: almost none of the saving requires it. **Accept with notes for memory ownership; revise before merge for the combined candidate.** The eight uninstrumented cold-burst pairs remain adverse and are not explained away by a favorable diagnostic. Keep PR #485 Draft and the broader write/memory objective active.

Production baseline: `1920e5141523a3e921d7c32f4e61889437e745e2`, Rust tree `ed64a1b1f99937f6bc45f1c923d98247cc366fbf`. Final ownership candidate: `490f43b429489c20768a06129ad126cf7b48fa6f`, Rust tree `39d469ea5f361438bf274938a2d94c688b78b030`. Ordinary release builds use rustc 1.98.1, default features, O3/debug0 and eight build jobs. Library, CLI and conformance identities were checked against fresh compiler artifacts from this checkout. The final harness matches all seven baseline input hashes exactly.

| Ordinary executable | Baseline SHA-256 | Candidate SHA-256 |
|---|---|---|
| Conformance | `a682bdcf9cff9c049b150565ae0c83c701993d1f2f7bea920e5cb0702dbfaa80` | `421c0e52fbd61797ff46f53e607441d7414acba49ad31e140ec6a3f33b73b462` |
| Native App Server CLI | `5c27b2671920f687ed520243e285b38fb3f7a15807ddc006905d59ced8f0b699` | `44f2fa7aa05f88c07d21f8f8dc5dfa0c18ce3d820177f04530af964366c58286` |

Pi Durable `1cedd32724abfcb0915f76cc61b6827e2c16dbad` and pi-rust `4056176bf4867f913099b101fd65b773895c99ba` were read-only source references. No reference product or DeepSeek was built or rerun. Pi's latest-only LiveDoc retires obsolete presentation; pi-rust's immutable cold locations share exact reader/range/digest/identity evidence. rustX borrows their ownership principle while retaining its old cuts and replay. It does not delete historical guarantees or replace SQLite with JSONL.

## What changed

`ContentBinding.content_ref` now holds `Arc<ContentRef>`. Native capture receipts, the existing immutable content-metadata map, prepared display bindings and replay retain the same exact reference owner. Handoffs no longer clone a full reference only to allocate a new Arc around it. Resolution shares that owner after the same existing validation; a changed reference or display binding cannot mutate an earlier cut. Display prefixes are constructed at their final length without first cloning and growing an obsolete path.

The Rust binding field type changes intentionally. Plain and structural JSON, public schema shape, safe-integer admission, ordering and SQLite schema 105 remain unchanged. There is no compatibility reader, persisted-format change, new global cache, new invalidation rule, query change or PRAGMA change. Current-storage checks and independent-domain failure boundaries remain authoritative. Sharing a locator is not evidence that current database bytes are healthy.

The full-wrapper prototype `5a26cfded` also shared path arrays. Its 128x32 census was 223.95 MiB versus the final simpler candidate's 224.05 MiB. Unique path arrays did not decline. Remove that wrapper and keep ordinary `Vec<String>` paths. The prototype's ordinary executables were built and identified, but no ordinary performance benefit is claimed for them.

## Actual owner census

Existing `issue-387-profile` gates a phase-only census, introduced at `ddbced4c2`. Each diagnostic product completes 4,096 real App Server requests. Count reachable unique Arc/Vec/Box allocations and actual capacities; do not add serialized repetition as heap memory. Current seeds, prepared-cache bodies not reached from replay, persistent-map internal nodes, nested row/model/tool payloads, identifier bodies, allocator rounding and SQLite are excluded. These are partial requested-byte measurements, not PSS partitions or headline timings.

| 128-session depth | Baseline known MiB | Candidate known MiB | Baseline binding parts MiB | Candidate binding parts MiB |
|---|---:|---:|---:|---:|
| 8 | 59.24 | 53.44 | 14.99 | 9.21 |
| 16 | 123.70 | 110.24 | 34.93 | 21.50 |
| 32 | 252.78 | 224.05 | 74.92 | 46.15 |

At depth 32, binding fragments fall from 30.27 to 15.60 MiB, raw binding vectors from 18.35 to 6.13 MiB, and path-vector storage from 22.40 to 18.16 MiB. The new 16,384 reference allocations cost 2.38 MiB and are included. Fragment counts are essentially unchanged, 120,246 -> 120,245. Replay records are 40,565 -> 40,580, reflecting asynchronous publication/coalescing; replacement checkpoints remain exactly 4,096 on both sides. All counted owners reach zero after unload. Capability snapshots already share their bodies; repeated capability serialization is not a new optimization or physical-memory saving.

## Ordinary native memory and growth

Four balanced independent pairs; each process loads 128 subscribed sessions and executes 32 turns each, sampling depths 8/16/32. GNU allocator, actual App Server over WebSocket, server-PID counters only. Side medians and medians of within-pair percentages are different summaries. Every negative result and per-pair delta is retained in [memory-summary.json](evidence/content-reference-owners-20261110/memory-summary.json). No statistical significance is claimed from four pairs.

| Endpoint, anonymous PSS MiB | Baseline side median | Candidate side median | Median paired change | Pair range |
|---|---:|---:|---:|---:|
| 128 subscribed empty | 337.85 | 335.93 | -0.15% | -1.77% to +0.98% |
| 128 x 8 turns | 661.92 | 656.52 | -0.76% | -1.52% to -0.13% |
| 128 x 16 turns | 842.64 | 823.11 | -2.42% | -3.23% to -1.80% |
| 128 x 32 turns | 1161.58 | 1116.37 | -3.59% | -4.06% to -3.24% |
| 8-to-32 growth, same processes | 498.06 | 461.88 | -7.51% | -7.62% to -6.91% |

All four populated endpoints improve at each sampled depth. Empty endpoints have mixed direction, so no fixed empty-session improvement is established. Seed-window CPU changes by a paired median -3.46%, and process writes by +0.006%. This is a memory-ownership change, not a write reduction.

A supplementary reverse-order 16-session, 64-turn pair is 215.71 -> 213.48 MiB anonymous PSS. It does not establish a precise deep-history percentage. All 1,024 primary memory-product backups and 32 deep-product backups pass independent SHA-256, integrity, FK, schema and exact ordered message checks. Each product also closes/reloads successfully. After unload the census is zero, while GNU process PSS remains high; those are different measurements and do not prove all physical pages were returned to the OS. Existing 30-second page-cache / 120-second statement-cache reclamation and allocator-return policy are unchanged.

## Original four ordinary scenarios

Four balanced pairs per cell, 32 ordinary products and 552 complete semantic request comparisons. No compiler overlaps were observed in sampled host monitoring. CPU includes the conformance driver and its endpoints have 10 ms granularity. Per-run P50/P95 ranges, first consumed content, sampled PSS, retained file growth, notification/native counts and writes through final close remain in the [raw summary](evidence/content-reference-owners-20261110/ordinary-summary.json) and [LF CSV](evidence/content-reference-owners-20261110/ordinary-per-run.csv). Product/driver boundaries differ from native capacity counters; do not compare their absolute CPU values as one workload.

| Case | P50 ms, side medians | Paired P50 | Paired P95 | Paired CPU | Hot process writes |
|---|---:|---:|---:|---:|---:|
| 8 setup turns | 13.75 -> 11.46 | -14.40% | +1.87% | -17.71% | +0.021% |
| 64 setup turns | 14.97 -> 13.61 | -9.16% | -0.93% | -10.83% | -0.134% |
| 32 KiB burst, cold first turn | 35.98 -> 40.16 | +9.43% | +9.43% | +0.00% | +0.000% |
| Paced 128-chunk SSE | 1578.01 -> 1587.26 | +0.13% | +0.13% | +3.57% | +0.000% |

The cold burst is an adverse result, not noise dismissed after the fact. Four prespecified additional balanced pairs test the same question without changing source or executable. Across all eight, paired P50 is +9.52%; six pairs are slower. Additional-four P50 is +9.88%. CPU endpoints are coarse; the eight-pair median CPU percentage is +12.5%, while raw values span 30-50 ms. Keep the original four and supplementary four separate in the [uncertainty record](evidence/content-reference-owners-20261110/burst-uncertainty-summary.json).

A small diagnostic reuses the existing profiler socket with process-CPU clocks instead of signal sampling or allocator replacement. Its four pairs reverse direction, CPU -17.53% and wall -16.05%. The probe adds a control thread/handshakes and uses different workspace directories. It does not identify the cause, override ordinary results, prove regression absence or justify an allocator/scheduling setting. No production change was made in response to that favorable diagnostic. The cold-burst tradeoff remains a final combined-candidate gate.

## Native active supplement

One supplementary pair per pattern, 128x8 seed followed by 256 active turns. Native PID only, zero sampled compiler overlap. These are not four independent CPU pairs. All 512 additional consistent backups pass exact content/order/count, hashes, integrity and FK checks. Slow consumer, browse, cancellation and three reload/unload cycles pass. Tool/cancel conformance also has a separate accepted functional pair, with three semantic request comparisons.

| Active pattern | CPU ms/turn before -> after | CPU change | P50 / P95 change | Process write change |
|---|---:|---:|---:|---:|
| fixed | 14.375 -> 13.945 | -2.99% | -2.49% / +3.56% | -0.018% |
| rotating | 13.047 -> 13.242 | +1.50% | +1.12% / +3.78% | +0.134% |

Rotation CPU and both tail-latency observations are adverse and remain visible. This set does not establish a large universal four-concurrency regression or equivalence.

## Correctness and next physical owner

Formatting and diff checks pass. All-target/all-feature Clippy denies warnings. Fresh all-feature CLI and supervisor builds complete; 1,664 distinct focused tests pass, three ignored, after these executables are built. Coverage includes byte-identical legacy/plain/structural binding encoding, COW old-cut/certificate isolation, exact reference-owner handoff, current-storage corruption, rollback/reopen, source bounds, runtime projection and agent execution. These are focused local checks, not a claim that all project tests or remote CI passed. Remote exact-final-HEAD status is recorded separately after push; do not wait for CI or merge.

No write saving is claimed. The earlier schema-105 commit-image-authenticated ledger identifies two remaining MessageId-specific block-header B-trees at 11.80/16.83/19.23/24.45 KiB per turn for 8/32/64/96 setup turns. This is their current WAL-frame plus authenticated main-page cost, not predicted net savings. The input ledger is preserved with its hash. MessageId kind selection has no production consumer; column consumers are the upsert and Tool publication rebinding reads/updates. The latter already receives the native Ledger position. Next, remove these duplicate coordinates and access structures while preserving the native binding, rowid BLOB locality, bounded queries and all failure/cut contracts. Keep the five commit roles, event facts, Source subtree reuse, full Statistics history and WAL/NORMAL policy.

The active objective remains broader than this memory mechanism. Do not mark it complete from this report.

## Evidence package

The [verified portable archive](evidence/content-reference-owners-20261110/owners-portable.tar.xz) retains every raw measurement/counter/request sample, all verification records, build/input identities and scripts. Its [member manifest](evidence/content-reference-owners-20261110/owners-portable.tar.manifest.json) authenticates all 910 members. Redundant database images are explicitly sampled (first, last and deep representatives); all original images, their hashes and a separate verified 2,811-member complete archive remain under the local evidence root. The [archive index](evidence/content-reference-owners-20261110/archive-index.json) records both archive hashes, scope and location. The portable archive is not described as containing every database image.
