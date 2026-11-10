# Bound Ledger coordinates remove redundant block header indexes

## Decision and source identities

**Accept with notes for the storage simplification.** Two redundant MessageId B-trees disappear, and process writes through final close fall in every ordinary pair. The change preserves the preceding reference-owner memory improvement. Hot-window history growth remains, and cold burst latency is adverse against the immediate baseline. Do not claim a general latency improvement, a solved CPU regression, or completion of the broader history-growth objective. Keep PR #485 Draft.

The original production baseline is `1920e5141523a3e921d7c32f4e61889437e745e2`, Rust tree `ed64a1b1f99937f6bc45f1c923d98247cc366fbf`. The immediate baseline includes reference sharing at `490f43b429489c20768a06129ad126cf7b48fa6f`, Rust tree `39d469ea5f361438bf274938a2d94c688b78b030`. The combined candidate is `bb132f76595f34409fa4b1d3bc1095ee22f71e53`, Rust tree `e193107b32cf0faafe6b418c1c4c28964aaf8256`.

| Candidate ordinary executable | SHA-256 |
|---|---|
| Conformance | `7c6225bf0e11d537d522386bd34d315fb9ab3c650d792e9abffcf5ae53c5fd95` |
| Native App Server CLI | `5d0e525df819d7ab3663f886a4a2a7f8a4d9ecfd37d2c148a9c791018de1951b` |

The fresh release build uses rustc 1.98.1, default features, O3/debug0 and eight jobs. Root library, conformance and CLI artifacts belong to this checkout and were rebuilt. All seven conformance input hashes match both preserved baselines. Source, harness and frozen measurement helpers remain unchanged through the main validation sequence. See [build identity](evidence/block-coordinates-20261110/build-identity.json) and [validation inputs](evidence/block-coordinates-20261110/validation-inputs.json).

Pi Durable `1cedd32724abfcb0915f76cc61b6827e2c16dbad` and pi-rust `4056176bf4867f913099b101fd65b773895c99ba` remain read-only source references. Neither they nor DeepSeek were rebuilt or run. Pi's integer storage identities inform the coordinate change. Pi's latest-only LiveDoc reclamation and pi-rust's authenticated cold locations remain lifecycle references; rustX retains its historical cuts and independent observation failures.

## One access coordinate with an exact identity binding

`observation_block_headers` retains its rowid layout, column order and bounded header BLOB. Reads, upserts and Tool publication rebinding use `(ledger_position, block_index)`. Delete `UNIQUE(message_id, block_index)` and `observation_block_kind_idx`; retain the numeric primary-key index and numeric kind index. All scoped native and observation event indexes remain.

MessageId remains binding evidence in the row. A single composite foreign key binds `(ledger_position, message_id)` to the existing unique parent index on `message_ledger(position, message_id)`. No replacement child index or authority cache is added. Removing MessageId entirely would lose the insertion-time check when a BEFORE INSERT trigger replaces the native identity. An isolated SQL counterexample and a production-path regression cover that boundary. The composite constraint also rejects a pair whose two coordinates independently exist but name different parents.

The exact position resolved while preparing one block is passed to its INSERT, removing the repeated scalar subquery. The next block performs a new native lookup. A trigger between blocks must still be observed. A numeric upsert cannot overwrite or heal a different stored MessageId; zero affected rows is an error. Tool rebinding retains its MessageId predicate and checks the current native position before updating.

Schema 106 rejects schema 105 explicitly. A relabelled old layout also fails shape validation through both open paths without changing file bytes. There is no migration or compatibility reader. Public JSON, protocols, external identities, five semantic commit roles, WAL/NORMAL, 2 KiB pages, 1,000-frame auto-checkpoint and idle reclamation policy are unchanged.

## Physical writes and checkpoint placement

One structural sample per history and side, with 32 hot turns. Page ownership is authenticated at WAL commit images; checkpoint page versions are matched to WAL images. These syscall payload measurements are separate from process write counters, retained dbstat layout and device writes. [Complete ledger](evidence/block-coordinates-20261110/storage-summary.json).

| Setup turns | WAL frames before → after | Hot WAL plus main KiB/turn before → after | Process MiB through final close before → after |
|---|---:|---:|---:|
| 8 | 7,119 → 6,969 | 559.53 → 548.42 | 23.81 → 23.26 |
| 32 | 7,641 → 7,461 | 623.85 → 592.22 | 39.93 → 38.56 |
| 64 | 7,902 → 7,653 | 634.17 → 633.93 | 62.21 → 60.02 |
| 96 | 8,489 → 8,176 | 725.04 → 680.25 | 87.23 → 84.55 |

The removed indexes account for 11.80/16.83/19.23/24.45 KiB per hot turn in the controls and zero in the candidates. Net savings differ because other pages and checkpoint placement also change. At 64 setup turns, main-file output rises by 507,904 bytes and checkpoint main syncs rise from seven to eight; that almost cancels the 515,896-byte WAL reduction. Policy is unchanged. At 32 turns checkpoint main syncs fall from eight to seven; at 96 they fall from nine to eight.

Every product still commits 160 write transactions in the hot window: 32 each for inbound acceptance, adoption, attempt/turn start, request start and final settlement. Each appends 256 native events, 32 catalog bindings and 224 definition rows. Native and Source new-node counts match per history, 285/319/351/352. These counts confirm subtree reuse and unchanged facts rather than a full-history rewrite or delayed terminal commit.

The measured hot window plus verification and final close is also preserved separately: 612.75→597.63, 695.50→663.88, 727.25→715.13 and 832.00→797.75 process KiB per hot turn. This includes verification/close and must not be relabelled as hot-only chat cost. Full initialized lifecycle totals above include history establishment.

## Ordinary performance retains adverse results

Forty-eight actual ordinary products cover four independent runs per revision/scenario in pairwise-balanced order. Three contrasts reuse those same products; they are not three independent experiments. There are 552 provider requests per revision. All complete ordered-content/options comparisons pass. No compiler was observed by the sampled host monitor. CPU includes the conformance driver and has 10 ms endpoint granularity. All per-run P50/P95, ranges, first consumed content, counters and adverse samples remain in the [three-way summary](evidence/block-coordinates-20261110/threeway-summary.json) and [CSV](evidence/block-coordinates-20261110/threeway-per-run.csv).

Percentages are medians of within-pair changes. They are not ratios of side medians, pooled turn samples or significance claims.

| Immediate baseline → storage candidate | P50 | CPU | Hot process writes | Process writes through close |
|---|---:|---:|---:|---:|
| 8 setup turns | +13.86% | +12.22% | −2.06% | −2.33% |
| 64 setup turns | −14.09% | −10.82% | +0.65% | −3.14% |
| Cold 32 KiB burst | +5.53% | 0% | −2.03% | −1.45% |
| Paced 128-chunk SSE | −0.99% | −6.67% | −1.52% | −1.36% |

Short-history P50 changes alternate direction, −42.90%/+73.74%/−34.79%/+62.51%. Cold burst P50 rises in all four immediate-baseline pairs, +1.80%/+35.25%/+9.25%/+0.11%. Its coarse CPU values do not isolate a cause. Preserve this result rather than claiming the reduced SQL/index work guarantees lower wall time.

Against the original baseline, combined P50/CPU changes are +5.79%/+15.23% for short history, −15.46%/−8.62% for long history, +1.13%/0% for burst, and −0.31%/+0.83% for SSE. Combined hot writes change −2.15%/+0.58%/−2.03%/−1.52%; through-close writes change −2.39%/−3.04%/−1.45%/−1.36%. The earlier eight adverse reference-owner burst pairs remain in their original report. This new mixed combined contrast does not prove that issue fixed.

In this ordinary series the short-to-long hot-write difference is about 103.25 KiB/turn in the original baseline and 120.56 in the combined candidate. Total maintenance is lower, but hot-window history growth has not flattened. Do not use the favorable lifecycle totals as a substitute for this remaining cost.

## Native memory and active sessions

Four balanced original-baseline/combined-candidate pairs use real App Server WebSocket sessions, ordinary GNU allocation and server-PID counters. Each loads 128 subscribed sessions and executes 32 turns each. All 1,024 consistent backups pass independent hashes, exact ordered content/counts, schema, integrity and FK checks. Close/reload cycles pass. [All memory samples](evidence/block-coordinates-20261110/memory-summary.json).

| Sampled anonymous PSS MiB | Original side median | Combined side median | Median paired change |
|---|---:|---:|---:|
| 128 subscribed empty | 341.19 | 337.51 | −1.19% |
| 128 × 8 turns | 663.45 | 657.30 | −1.24% |
| 128 × 16 turns | 845.47 | 826.92 | −2.35% |
| 128 × 32 turns | 1161.70 | 1123.37 | −3.51% |
| Same-process 8-to-32 growth | 500.33 | 464.96 | −7.15% |

Every populated endpoint falls in all four pairs. Empty-session direction remains mixed. The depth-32 paired changes range from −3.25% to −7.82%; one control has a substantially higher endpoint, so do not describe the largest delta as the expected saving. Seed CPU falls by a paired median 3.39%, and writes by 1.46%. A supplementary reverse-order 16×64 pair is 217.68→205.07 MiB, with all 32 backups independently verified. It is not a precise deep-capacity percentage. Process PSS after unloading remains high and is not a live-object census.

One supplementary pair per active pattern starts from 128×8 and executes 256 turns. All 512 additional backups pass exact content/order/counts, hashes, schema, integrity and FKs. Slow-consumer, browse, cancellation and reload checks pass; no compiler is observed in the active windows. [Native supplement](evidence/block-coordinates-20261110/native-summary.json).

| Active pattern | CPU ms/turn before → after | CPU change | P50 / P95 change | Process write change |
|---|---:|---:|---:|---:|
| Fixed four | 14.219 → 13.828 | −2.75% | −0.52% / +0.23% | −3.64% |
| Rotating four | 13.555 → 12.656 | −6.63% | −1.80% / −12.74% | −1.49% |

These supplements do not reproduce the previous large four-concurrency CPU increase. One pair per pattern cannot establish universal regression absence. Fixed P95 remains slightly adverse. Tool/cancel conformance has an additional accepted functional pair for each case.

## Correctness and remaining work

The exact candidate Rust source passes Fmt, all-target/all-feature Clippy with warnings denied, fresh all-feature CLI/supervisor prerequisites, and 1,670 distinct focused tests with three ignored. Overlapping filters produce 1,836 successful executions. An initial filename-based filter selects zero tests; all six actual source-read cases pass in the broader observation and SQLite groups. Earlier version-assertion and test-compilation failures are retained. Coverage includes composite identity/trigger checks, corruption refusal, numeric query plans, explicit schema gates, rollback, old cuts, bounded reads, frozen/retry identity, canonical settlement and observation isolation. These are focused local checks, not all project tests or completed remote CI.

The reference-owner mechanism and this coordinate/index change are the two retained mechanisms. Neither adds a global cache, an authority shortcut, sparse Statistics replay, a smaller Replay window or a new checkpoint policy. The latter's value is removing persistent maintenance while simplifying ownership; it is not a claim of a large speedup.

Next, inspect actual Replay event layout and cut/frontier owners. The preceding diagnostic census identifies 67.06 MiB of event envelopes/nested vectors and 53.91 MiB of frontier-related allocations at 128×32. Those are partial requested allocations from the preceding census, not a new combined-candidate PSS partition or wholly removable bytes. Empty Replay cannot explain the remaining empty-session baseline. Cold-burst attribution and remaining history-dependent page maintenance remain open. The full goal is active.

The remote snapshot for the production commit has #485 open, Draft and mergeable, with only the notification job recorded successful at that observation. Required exact-final-HEAD correctness gates remain unknown; do not wait for CI or merge as part of this iteration.

## Evidence retention

The [verified portable archive](evidence/block-coordinates-20261110/block-coordinates-portable.tar.xz) retains every measurement, raw per-turn/process counter, provider request, negative result, verifier record, build/helper identity and backup hash, plus representative native database images. Its [member manifest](evidence/block-coordinates-20261110/block-coordinates-portable.tar.manifest.json) authenticates 802 files. The separate full archive authenticates 2,329 files and retains all consistent native backup images. ELF/dependency trees and private runtime/workspace roots are excluded; originals remain in the local evidence root. The [archive index](evidence/block-coordinates-20261110/archive-index.json) records exact hashes and scope. The portable package is not described as containing every database image.
