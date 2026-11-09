# History-growth plan: memory ownership and persistent index keys

## Decision and scope

Prioritize two separately reviewable mechanisms: compact common lifecycle keys in the two global event-kind indexes, and remove duplicate immutable canonical bodies across the current conversation/model-input boundary. First measure live owners to choose the precise memory change. These are candidates, not measured improvements. Do not redesign the entire journal or replace bounded history with replay.

This document responds to the request for a source-based plan. No new production optimization is retained. `87a262e15991cc0e992ccba5536464dd18b387d3` was reverted by `ebe95b186`; the resulting `src` tree is `75a6f89ac3faf591ae763664cc728f45dbd46e0c`, identical to `43ab0356d9fcd4d5f6485fa73b9d15ab2618d67f`. The current schema remains 102. An unretained key-format sketch and an independently discovered outcome-read defect are archived locally, outside production. The sketch did not pass Clippy and is not a validated candidate.

Reference repositories were inspected without building, running or modifying them:

| Repository | Exact local revision |
|---|---|
| Pi Durable | `1cedd32724abfcb0915f76cc61b6827e2c16dbad` |
| pi-rust | `4056176bf4867f913099b101fd65b773895c99ba` |

The pi-rust revision here is the earlier measured reference, not the later `b1bde3e3` revision. No Pi, pi-rust or DeepSeek benchmark was rerun.

## What the references actually suggest

Pi Durable's `packages/durable/src/harness/live.ts` defines a latest-only LiveDoc. Idle and tool-boundary bases are small; ended generation/tool presentation is removed. `storage/sqlite/storage.ts` deletes superseded current-only document revisions when committing a new base. This is a lifecycle rule, not merely a faster serializer. Numeric internal identities and semantic write batches limit repeated persistent bookkeeping.

pi-rust's `crates/rpi-harness/src/session/jsonl/cold_frames.rs` replaces eligible resident payload slots with immutable `Location` references after a proven append. Locations bind reader, offset, length, digest and identity; reads authenticate the selected bytes. Read plans own selected references rather than a second full history. `LiveNulls` preserves distinctions that JSON roundtrips would lose. Metadata still grows with history: this is not constant-space history.

Its `session/jsonl/progress.rs` keeps a reclaimable progress companion and explicit commit/recovery boundaries. The OS append implementation uses `write_all` and `flush`, not per-append `sync_all`. Pi Durable and rustX both use WAL/NORMAL; rustX does not fsync every transaction. These differences do not justify weakening rustX's current settlement contract.

Apply the ownership and lifecycle principles. Do not transplant latest-only deletion into domains that promise old cuts, and do not replace SQLite with JSONL as part of this iteration.

## Write evidence and priority

The retained schema-102 diagnostic products contain 32 measured short turns after each setup history. Page owners are authenticated at WAL commit images, with checkpoint page versions matched to WAL images. There is one structural sample per cell. These are syscall payload counts, not SSD traffic, ordinary latency measurements or statistical estimates. See the [derived ledger and raw-input hashes](evidence/history-growth-plan-20261110/write-ledger-summary.json) and the [preceding methodology](2026-11-09-write-ledger.md).

| Component | WAL plus main-file KiB/turn, 8 setup turns | 64 setup turns | Interpretation |
|---|---:|---:|---|
| All event indexes | 163.16 | 188.49 | Largest indexed maintenance group |
| Two global kind indexes, included above | 33.48 | 52.37 | +18.90 with history; narrow first experiment |
| Content frontiers and layout | 50.00 | 64.59 | Important, but field deduplication alone failed its gate |
| Native input manifest and Request Source | 46.11 | 55.10 | Old subtrees already shared; no full-history rewrite |
| Request Trace catalog | 24.08 | 28.23 | Seven repeated definition rows per request in this fixture |
| All components | 575.20 | 668.59 | +93.39; categories above are incomplete and overlapping |

The five semantic write commits per turn, eight native events per turn and 32 request bindings per hot window remain. Catalogs add 224 definition rows in each 32-turn window. The two kind indexes are a subset of event indexes and must not be added again. The original 131.1 KiB/turn process-counter gap comes from another measurement series; its unexplained remainder cannot be assigned to a table using this ledger.

### First write candidate: compact the existing two global kind keys

`schema.rs` stores full native event-type text in `events_kind_idx`. `auxiliary/schema.rs` separately indexes the independent `observation_kind` in `events_primary_trace_kind_idx`. Keep both independent sources and both indexes; replace common lifecycle text keys with small private typed codes. Leave public kinds and payloads unchanged. Retain all scoped indexes and rare-query coverage initially.

An offline integer-key layout probe on a copied 64+32-turn database reduced each global index from 11 bulk-rebuilt control pages to 5. The original incrementally maintained indexes had 13 pages each. The [probe](evidence/history-growth-plan-20261110/kind-index-prototype.json) uses SQLite 3.45.1, not the production writer. It measures retained layout only. It does not establish WAL savings, CPU cost or the footprint of a different BLOB-key implementation.

Implementation gate:

1. One explicit key mapping, disjoint typed handling of unknown/corrupt values, and the normal pre-1.0 schema gate. Prefer built-in SQLite expressions over connection-specific functions or another persisted key column.
2. Exact expression/parameter agreement, independent native/Trace corruption behavior, forward/reverse sequence bounds and current query-plan/VM-step tests. An indexed lookup must remain indexed rather than scanning the old history.
3. Measure WAL frames, checkpoint bytes, changed pages and statement memory. Count additional key-expression work; smaller B-trees can still cost more CPU.
4. Retain only a material physical-write reduction with acceptable CPU and cold/history reads. The entire two-index cost is only 5.8%–7.8% of the total: this is not a route to a several-fold write reduction by itself.

### Rejected experiment: consolidate five outcome field sources

The prototype stored one terminal Source body and four bounded field spans instead of five independently indexed bodies. Frontier rows dropped from 480 to 352 per 32 hot turns, but net logical savings were only about 5.3 KiB per window. Span metadata and a new failure dependency increased representation complexity.

| Setup history | Schema-102 WAL plus main KiB/turn | Prototype | Change |
|---|---:|---:|---:|
| 8 | 575.20 | 570.09 | −0.89% |
| 32 | 641.85 | 639.02 | −0.44% |
| 64 | 668.59 | 666.34 | −0.34% |
| 96 | 718.24 | 717.03 | −0.17% |

These structural products do not show a reduced history slope. Reject the format rather than expanding it. The prototype's tested source passed 1,533 focused library tests and all-target/all-feature Clippy, but a later unbuilt missing-capture guard is not covered by those results. No ordinary timing improvement is claimed. [Build identities](evidence/history-growth-plan-20261110/build-identities.json) distinguish its executable from the newly rebuilt baseline; the existing structural schema-102 records use the earlier archived executable from the write-ledger report.

## Memory evidence and priority

The source confirms allocation boundaries, not their current PSS contribution:

| Owner/boundary | Current behavior | Next action |
|---|---|---|
| Ledger and canonical Surface | `MessageLedger` owns `Arc<MessageBlock>`; `CanonicalSelection` shares those allocations | Preserve; do not count the two references as two bodies |
| Model input | `ModelInputMessage::Canonical` owns a `MessageBlock`; staged assembly clones it; `InputNode.actual` then retains an `Arc<ModelInputMessage>` | Count distinct body allocations and lifetimes across Ledger, selected input and frozen request |
| Published message head | `SharedSnapshot.messages` is an `Arc<Vec<MessageBlock>>` through `CowDomain`; mutation can detach the vector and bodies | Count changed-domain copies, prepared capture and replay retention; native head is already limited to eight |
| Descriptor/transcript/replay | Existing persistent rows and shared events; one replay ring, 4,096 events/16 MiB encoded budget; cursor-only subscribers | Preserve delivery contract; count physical allocations, because encoded charge is not heap size |
| SQLite | Shared connection owner, existing idle page/statement reclamation | Measure SQLite counters separately from allocator retained pages; do not claim two connections per session |

`src/model/input.rs::assemble_selected_model_input()` reuses the historical manifest and copies new staged canonical bodies. This is not a full-history clone on every turn. `selected_input.rs::InputNode.actual` already shares the resulting input values across persistent prefixes. The remaining question is sharing the exact body across layers, not adding another prefix cache.

The 480.75 MiB 128×8 result is an older `e7289ac18` measurement. It cannot identify current live owners. The checked-in App Server capacity driver still speaks protocol 52; production speaks protocol 76. Update that fixture to current thread/start, thread/resume and turn identity contracts before quoting a new 128-session result.

### First memory candidate: one immutable canonical body owner

Before changing types, record unique allocations and retained bytes at quiescence. Separate Ledger/Surface, current model selection, in-flight request/retry, prepared observation, replay and transport. Distinguish bodies from reference arrays and capacities. Include a body-size-scaled fixture so tiny strings do not hide duplication.

If the confirmed canonical/model boundary is material, let unmodified canonical input share its exact immutable body owner through request preparation. A transformed input gets a new Derived value; it must never modify a canonical body or borrow a newer cut. Extend sharing through the eight-message observation head only if it contributes enough retained bytes to justify the API change. Do not add a parallel compatibility path.

Only after this owner audit, consider pi-rust-style cold payload references as a separate future design. That would require explicit bounded hydration, current-byte authentication, owner/epoch binding, and exact non-JSON-roundtrip semantics. It must not silently add a full-history database read to every request. This larger lifecycle change is not bundled with the first two candidates.

## Ordered work and acceptance

1. Calibrate the current App Server fixture. Measure empty and 8-turn states at 1, 16 and 128 sessions, plus 32/64-turn growth in a small fixed session set. Keep the same subscriber, prompt and catalog. Separate immediately settled, page-cache-idle and statement-cache-idle endpoints. Use the ordinary allocator for PSS/RSS and a separate ownership diagnostic for unique live allocations.
2. Validate compact global kind keys in isolation. Reuse histories 8/32/64/96 with 32 hot turns, authenticated WAL replay and unchanged checkpoint policy. Report retained dbstat growth separately. Preserve every adverse sample.
3. Implement the largest confirmed duplicate-body boundary as a separate commit. Require exact old-cut immutability, transformed input isolation, frozen/retry identity and release after the actual commit. Demonstrate which owner disappears and when it releases.
4. Compare the final rustX candidate against its own saved baseline. Preserve burst and paced SSE regressions; include cold/history reads and fixed/rotating four-concurrency CPU. No reference-product reruns. Headline timings, if needed for the final decision, use ordinary binaries without tracers and balanced order, not the structural samples.

Keep cancellation/start-failure no-dispatch, canonical atomic ordering, terminal-last, old cuts, bounded reads, resync/slow subscribers, rollback/process death/reopen/archive and independent domain failure checks. A retained storage format needs exact schema-gate and query-plan regressions. Required exact-HEAD CI remains a merge gate; the requested plan does not wait for it and does not merge #485.

Retained history necessarily occupies space, and selecting a growing model context has unavoidable work. The target is smaller per-fact storage, fewer repeated page modifications, one body owner where semantics allow it, and bounded optional working sets. Do not promise zero historical growth or a new Pi multiplier.

Defer shared Trace catalog indexes, checkpoint-threshold experiments and a broad cold-runtime redesign until these two candidates are decided. Reject small gains that introduce substantial ownership, invalidation or decoding machinery. Keep the existing SQLite idle policies, full self-contained Statistics history, index query contracts and long-output behavior.
