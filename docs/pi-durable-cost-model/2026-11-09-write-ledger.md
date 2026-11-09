# PR #485 write-cost ledger

## Identity and scope

The production baseline is `b39c8fbd719e83ceb99ade9f23893face56630e0`. The original benchmark correction is `63f2120a9c3ffb913fb61dfed3ca30ec35aa7d35`. The original 64 raw product records, CSV and summary were revalidated from each raw run. The check reproduced every exported metric and confirmed the 552 rustX/Pi core request comparisons. It authenticated the rustX executable `4043408edbfa64a0ed95a62dd0f01eaa61030012a829dfbb88fc2254a93c55f0`, its release library `0dd245328a2f5d67f61bf8fa3d38e044b15fc369996d98c2f7776e91f2c33a3d`, the pi-rust executable `4762ec079d86ce548d38a2840b2970ac6f8b093ada210692479b0c384b7a7017`, and the recorded Pi and DeepSeek source and generated inputs.

The historical runtime host was Linux x86_64 with an i9-14900K, ext4/NVMe and affinity to CPUs 2, 4, 6 and 10. Rust used rustc 1.98.1 and ordinary optimized builds. Pi and DeepSeek used Node 24.22.0. Pi used its previously emitted AI package and native Durable adapter. DeepSeek used the pinned `687f282e` SDK source launch with Zstd JSONL. The original manifest records the Node version. It lacks a byte hash for the original Node executable, so the currently installed Node hash is supplemental. The recovered suite preserves each native prompt and tool catalog separately. The cross-product rustX/Pi oracle checks ordered conversation and request options. The other products have independent complete-history, reply and reopen checks.

## Persistence contracts

| Product | Writer policy and persistent state | Measurement boundary |
|---|---|---|
| rustX | Bundled SQLite 3.53.2, WAL/NORMAL, 2 KiB pages and 1,000-frame auto-checkpoint. Native journal, exact frozen history and independently available inspection domains. | Committed settlement plus Runtime Client live observer. |
| Pi Durable `1cedd327` | Node SQLite 3.53.4, WAL/NORMAL, 4 KiB pages and 1,000-frame auto-checkpoint, verified in the native adapter. Internal numeric identities and cached connection statements. Semantic writes can group answer, task outcome and submission settlement. The generation stores the exact entry cutoff. LiveDoc keeps a latest view with retired revisions. | Native Durable completion plus watchEvents. |
| pi-rust `4056176b` | The measured RPC build uses sequential JSONL and a reclaimable progress companion. Owned reducer updates, batching and recovery barriers. The inspected append code in both `4056176b` and `b1bde3e3` uses `write_all` and `flush`. | RPC agent_settled after event delivery. Recovery barriers and power-loss synchronization have distinct semantics. |

The same 1,000-frame default represents approximately 2 MiB of rustX page payload and 4 MiB of Pi page payload. The follow-up comparison uses unchanged checkpoint policy. The archival products retain different native envelopes, observation surfaces and persistence commitments. DeepSeek reports SDK idle and records native close separately.

## Authenticated physical write ledger

The structural products reuse the existing diagnostic phase socket and ordinary SQLite writer. The extended histories use the same text and 32 hot turns. The largest setup is 96 turns because this driver verifies at most 128 terminal facts. The phase socket pauses the diagnostic driver for a short read-only layout inventory. Its samples provide structural counts only. Uninstrumented timing uses separate processes without the socket or tracer.

The decoder replays successful WAL page writes into a shadow database, validates rolling checksums and salt/offset continuity across reuse epochs, and checks the reconstructed final column totals and page ownership against a live snapshot. WAL ownership comes from dbstat at the committing image. Main-file page versions are matched to authenticated WAL page images by page number and digest. Missing or ambiguous owners are counted separately. dbstat payload, unused bytes, freelist and overflow are retained layout observations. Transaction row changes are net committed row/cell differences. They cannot count an update that was overwritten within the same transaction. The diagnostic sockets, pipes, catalog files, scratch or auxiliary descriptors, bootstrap rollback journal, WAL and main file are separately recorded.

| Setup turns | Write commits | WAL frames | Distinct WAL pages | WAL epochs | WAL KiB/turn | Main KiB/turn | Process write_bytes KiB/turn | Writes through writer close MiB |
|---|---|---|---|---|---|---|---|---|
| 8 | 160 | 7819 | 895 | 8 | 494.4 | 126.1 | 666.8 | 26.14 |
| 32 | 160 | 8488 | 973 | 8 | 536.7 | 146.2 | 749.8 | 44.25 |
| 64 | 160 | 8831 | 1037 | 8 | 558.4 | 160.4 | 799.8 | 69.62 |
| 96 | 160 | 9323 | 1140 | 9 | 589.5 | 200.7 | 899.4 | 97.63 |

The traced 8-to-64 difference is 64.0 KiB/turn of WAL and 34.3 KiB/turn of main-file writes, together 98.3 KiB/turn. The original ordinary process-counter gap is 131.1 KiB/turn. Approximately 32.8 KiB/turn of that original gap remains unassigned by this diagnostic comparison. The separate traced process-counter gap is 133.0 KiB/turn, leaving 34.7 KiB/turn outside the two syscall payload totals in the traced products. Kernel-attributed bytes and syscall payload lengths have different accounting boundaries. Neither is physical device traffic or evidence of a blocked latency path.

The hot 8- and 64-history products each record eight main-file syncs and sixteen WAL syncs alongside 160 write commits. The phase ledger distinguishes startup, opened schema, seed history, hot turns, verification/reads, shutdown/close, reopen verification and cleanup. The hot interval contains no writes to the Session Catalog file in these traces. `wchar`, `syscw` and `cancelled_write_bytes` remain separately recorded.

| Native commit owner | Commits/turn | WAL KiB/turn at 8 | WAL KiB/turn at 64 |
|---|---|---|---|
| inbound_acceptance | 1 | 23.71 | 21.82 |
| inbound_adoption | 1 | 97.44 | 105.47 |
| attempt_turn_start | 1 | 59.50 | 66.33 |
| request_start | 1 | 137.40 | 152.26 |
| final_settlement | 1 | 176.36 | 212.52 |

The short workloads have no separate intermediate publication or post-commit observer write transaction. The subscriber remains enabled. The final commit is the largest writer and includes final publication and canonical/terminal settlement facts.

| Component | 8 WAL + main KiB/turn | 64 minus 8 WAL + main KiB/turn | 8 → 64 retained-page growth per 32 turns | Semantic owner | Decision |
|---|---|---|---|---|---|
| events and native/observation indexes | 199.6 + 45.2 | +24.6 / +12.7 | 322 → 325 | Native Journal and independent Trace selection | Scope auxiliary probes and retire the unused global Trace index |
| canonical and transcript rows | 119.6 + 27.5 | +13.7 / +6.8 | 142 → 142 | Ledger, Surface and independent bounded transcript | Retain the current owners |
| request owner/snapshot/model | 41.0 + 13.1 | +3.6 / +1.8 | 80 → 74 | Frozen request and request/settlement proof | Retain |
| request manifest and Source | 35.0 + 18.8 | +7.7 / +3.3 | 180 → 190 | Exact frozen input and bounded request inspection | Retain current subtree reuse |
| content frontiers and segment layout | 43.1 + 7.8 | +7.5 / +5.9 | 25 → 22 | Independent byte sources and visible bounds | Retain the exact range layout |
| other btrees / page 1 | 38.0 + 7.1 | +4.9 / +2.4 | 18 → 15 | Lifecycle, publication, sequence and database metadata | Retain |
| request Trace catalog | 18.1 + 6.7 | +2.0 / +1.4 | 17 → 16 | Original request binding and Tool definition evidence | Defer immutable catalog sharing |
| unattributed | 0.0 + 0.0 | +0.0 / +0.0 | 0 → 0 | Unmatched page version or free page | Keep explicit |

The 8-history column is a measured low-history anchor for fixed per-turn cost. It is not an intercept extrapolated from page writes. The five transaction roles and their 32 occurrences are fixed in all four runs. The page-ownership table and the semantic-commit table describe overlapping classifications of the same WAL writes and must not be added together.

| Setup turns | New native nodes | New Source nodes | Source metadata bytes | Source pages / bytes | Catalog bindings / definition rows | New native events | Retained DB KiB above opened empty |
|---|---|---|---|---|---|---|---|
| 8 | 285 | 285 | 56480 | 229 / 151112 | 32 / 224 | 256 | 2056 |
| 32 | 319 | 319 | 64334 | 228 / 150694 | 32 / 224 | 256 | 3204 |
| 64 | 351 | 351 | 71161 | 224 / 143456 | 32 / 224 | 256 | 4772 |
| 96 | 352 | 352 | 71407 | 229 / 151698 | 32 / 224 | 256 | 6376 |

Source page counts stay near 225 while new path nodes rise from 285 to roughly 351. The data is consistent with the retained subtree reuse in manifest and Source construction. It provides no evidence of a full-history rewrite. Event bodies account for about 110,000 native JSON bytes per 32 turns, with about 312,000 to 315,000 independently derived column-value bytes. There are 256 appended events, 32 request bindings and 224 definition rows in each history. In the 8-history probe, committed row changes include 32 inserted and 32 updated request snapshots, 64 inserted and 32 updated observation headers, 512 inserted and 32 deleted frontiers, and 160 updates to the store counter row. Column lengths and per-owner page payload writes use separate units.

The two chosen event indexes account for 34.4 KiB/turn of traced WAL and checkpoint payload at 8 history and 52.1 KiB/turn at 64. The entire request Trace catalog group contributes about 24.8 and 28.2 KiB/turn, including its independently retained per-request binding. Index narrowing preserves the real auxiliary probes with less ownership complexity than a shared catalog. The Source digest index has a measurable growth contribution and remains paired with its current rowid/blob representation. No new context estimator cache or checkpoint policy was introduced.

## Retained storage changes

1. Limit `events_auxiliary_call_id_kind_idx` to non-null top-level call IDs. Every production probe of this index constrains the same expression by equality. The three SQL shapes retain an indexed search, including a covering plan where applicable. The 100-call growth test finds 200 entries from the two independently indexed call facts. Per-request events with no top-level call ID retain their existing native and request indexes.
2. Remove `events_trace_kind_idx`. No production SQL read depends on the broad observation-kind index. The former negative-control test still installs and uses a test-only copy to exercise an intentionally non-scalable access path. Trace queries retain their scoped native and observation-owner paths, historical bounds, current-byte checks and independent corruption handling.

Schema 102 rejects schema 101 before decoding facts or modifying the database. The regression reconstructs the exact preceding index DDL and verifies rejection through both open APIs with identical file bytes. The public identifiers and five measured commit roles retain their existing meanings. The candidate also includes a separate test-only Trace correction for the pre-existing 64 KiB range assertion. An unaligned range can cross nine 8 KiB raw argument rows, with the same byte budget and raw spelling verification.

| Setup turns | WAL frames before → after | WAL + main-file KiB/turn before → after | Closed database pages before → after |
|---|---|---|---|
| 8 | 7,819 → 7,350 | 620.55 → 575.20 | 1,278 → 1,267 |
| 32 | 8,488 → 7,897 | 682.97 → 641.85 | 1,852 → 1,836 |
| 64 | 8,831 → 8,137 | 718.85 → 668.59 | 2,636 → 2,611 |
| 96 | 9,323 → 8,683 | 790.21 → 718.24 | 3,438 → 3,410 |

All four candidate diagnostic runs retain five hot write commits per turn, 256 native hot events, 32 request catalog bindings and 224 catalog definition rows. Neither target index writes a page during these text-only hot windows. The 8-to-64 physical growth persists at 93.4 KiB/turn. This records a reduction in the fixed writer burden and a modest decrease in the history slope.

## Ordinary fake-provider regression

The candidate production source is `a480ccbc9e97a113e67b8d1482fbd568e59a0460`, with tree `6258002a2fb41cc05bc981d40b3cca49b25fc4e1`. Its ordinary release executable SHA-256 is `a1fa2e697aeb451458940577dc88ba3ee72a1eb390ba7efccdcd40690d8c5497` and the library SHA-256 is `4eaa83a37cd3160898b19f9656fafc9144d8e6d2977cbc1f218d455b893e8868`. The release build predates three changes confined to Rust unit-test sources. The evidence archive records the exact release-build tree and the test-only diff paths.

On the same host and CPU affinity, four fresh independent pairs per scenario used alternating baseline/candidate order, the local HTTP/SSE Fake Provider and the normal live subscriber. Both sides ran the same text and response fixtures. All 32 product samples passed native history, reply, observer, final-close and reopen checks. The 552 paired request checks preserved the complete semantic request envelope after fixture workspace normalization. No paired samples were discarded. Pi Durable, pi-rust and DeepSeek were reused from the independently revalidated archival run.

The latency columns give the median and range of the four per-run percentiles in milliseconds. Each short run has 32 measured turns. Burst and paced SSE have one measured turn per run, so both percentiles coincide. Paired deltas are medians of four within-pair changes. They can differ from the ratio of side medians.

| Scenario | Baseline P50 [range] | Candidate P50 [range] | Baseline P95 [range] | Candidate P95 [range] | Paired P50 / P95 |
|---|---|---|---|---|---|
| 8-turn history | 11.01 [9.08–12.72] | 11.75 [7.15–13.20] | 15.32 [15.13–17.19] | 17.32 [12.79–17.97] | +4.96% / +7.54% |
| 64-turn history | 12.32 [8.13–14.22] | 10.22 [7.98–14.31] | 18.33 [12.24–18.64] | 17.02 [13.02–19.33] | -12.80% / -5.12% |
| 32 KiB burst | 31.07 [30.36–32.18] | 31.60 [29.36–32.41] | 31.07 [30.36–32.18] | 31.60 [29.36–32.41] | -0.04% / -0.04% |
| 128-chunk paced SSE | 1551.17 [1544.38–1557.11] | 1549.44 [1536.76–1571.94] | 1551.17 [1544.38–1557.11] | 1549.44 [1536.76–1571.94] | -0.08% / -0.08% |

The following are side medians except for the paired write change. CPU uses Linux process ticks and includes the in-process driver. PSS is the maximum sampled value in each hot window before taking the median across runs. Close writes span pre-initialization through final writer close. Retained main-file size subtracts each opened-empty layout.

| Scenario | First consumed content, ms | CPU ms/turn | Hot write_bytes KiB/turn | Paired hot writes | Close write_bytes MiB | Hot sampled PSS MiB | Closed main KiB above empty |
|---|---|---|---|---|---|---|---|
| 8-turn history | 11.00 → 11.74 | 14.38 → 15.16 | 667.7 → 607.7 | -8.99% | 26.29 → 24.35 | 89.22 → 88.98 | 2056 → 2036 |
| 64-turn history | 12.31 → 10.21 | 15.62 → 13.59 | 812.6 → 739.6 | -8.95% | 69.95 → 64.32 | 93.75 → 93.90 | 4771 → 4722 |
| 32 KiB burst | 31.04 → 31.59 | 30.00 → 30.00 | 608.0 → 592.0 | -2.63% | 2.19 → 2.15 | 81.38 → 81.32 | 250 → 250 |
| 128-chunk paced SSE | 127.15 → 126.65 | 120.00 → 120.00 | 804.0 → 788.0 | -1.99% | 2.33 → 2.29 | 81.45 → 81.24 | 180 → 180 |

| Scenario | Native journal sequence / canonical records / requests at final cut, both builds | Hot subscriber notification records, baseline → candidate ranges |
|---|---|---|
| 8-turn history | 320 / 81 / 40 | 346–354 → 350–356 |
| 64-turn history | 768 / 193 / 96 | 350–353 → 345–350 |
| 32 KiB burst | 8 / 3 / 1 | 10–11 → 9–10 |
| 128-chunk paced SSE | 8 / 3 / 1 | 23–25 → 24–24 |

Notification envelopes can be coalesced by the live observer. Canonical records and journal/request counts were equal in every pair. First content was observed on every measured turn.

Writes fell in all sixteen pairs. The 64-minus-8 history gap in the new ordinary side medians fell from 144.9 to 131.9 KiB/turn. The ordinary baseline for this paired series is separate from the original 131.1 KiB/turn gap. The 8-history P50 and P95 rose in three of four pairs, with paired medians of +4.96% and +7.54%. The 64-history P50 had changes from −31.59% to +33.85%, with two faster and two slower pairs. Its lower median does not establish a dependable latency gain. Paired CPU medians were +2.12%, −5.12%, 0.00% and +4.17% by scenario. The single-turn CPU readings have 10 ms resolution. PSS changed by less than 1 MiB in the scenario medians.

Burst and paced SSE settled within 0.1% of the prior rustX build by paired median. The candidate side medians were 31.60 ms and 1,549.44 ms, with 30 and 120 ms CPU. The independently verified archival Pi Durable results for those cases were 88.09 and 1,750.49 ms and 110 and 650 ms CPU. This comparison preserves the original long-output and streaming result under the respective native contracts. The archive reports the original four products separately.

## Correctness and disposition

The local checks covered the SQLite gate and query plans, old cuts, bounded Trace reads, exact frozen requests, cancellation and start failures, durable release and settlement, rollback and process death, independent observation failures, resync, archive and reopen. The focused all-feature library run passed 1,913 tests with three ignored. External contracts, durable, process and provider targets passed 31, 151, 68 and 188 tests respectively, with six provider tests ignored. The all-targets/all-features Clippy check with warnings denied passed, as did the 48 Python controller tests and the seven Rust request-oracle tests. The prior full run for the benchmark-correction HEAD identified the stale Trace assertion independently of this storage candidate.

The local build commands were `cargo test --locked --all-features --lib --no-run` and `cargo test --locked --all-features --test contracts --test durable --test process --test provider --no-run`. The library test binary selected SQLite, runtime client, agent execution, scripted agent, App Server, durable boundary and archive suites, excluding scale tests. The four external binaries ran their complete default test selections. Static verification used `cargo clippy --locked --all-targets --all-features -- -D warnings` and `git diff --check`.

**Accept with notes for the two index changes.** The mechanism reduces measured page traffic and ordinary process writes while preserving the indexed read and settlement contracts. The short-history latency result remains adverse in this small sample. The production sources in this report commit match the measured candidate. PR #485 stays Draft. Its exact-HEAD Merge verification verdict is tracked in [PR checks](https://github.com/Caismis/rustX/pull/485/checks).

Remaining write costs reside primarily in event/canonical indexes, transcript and content frontiers, and the growing request-manifest path. Immutable Trace catalog sharing and exact fixed-context accounting were deferred. The retained scope is the two index changes above.

The [paired per-run CSV](evidence/write-ledger-20261109/paired-per-run.csv) contains every product observation. The [evidence archive](evidence/write-ledger-20261109/evidence.tar.xz) and [checksums](evidence/write-ledger-20261109/evidence.tar.manifest.json) retain the new raw products and exact request comparisons, the physical and logical write ledger, and the validated original manifest, CSV and raw-file identities. Full syscall capture and the original raw products remain in their local measurement directories.
