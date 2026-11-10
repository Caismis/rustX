# Canonical coordinates experiment

**Redesign within the verified scope. The measured candidate was rejected.** Its stable write reduction did not justify the additional owner-read work alongside the adverse CPU observations. Production source and tests were restored to the schema-102 baseline. The two earlier index optimizations remain part of that baseline.

## Source and measurement identities

The baseline is `1bce126843d79d714b4a7ead7f1df6aacc1d644e`. Its executable SHA-256 is `a1fa2e697aeb451458940577dc88ba3ee72a1eb390ba7efccdcd40690d8c5497`, with release library `4eaa83a37cd3160898b19f9656fafc9144d8e6d2977cbc1f218d455b893e8868`. Its production implementation at `a480ccbc9e97a113e67b8d1482fbd568e59a0460` matches the baseline. The measured build tree differs in three Rust unit-test files and documentation. The identity check validates those paths and the executable bytes.

The measured candidate is `8f0ca1cf2b5265276cae81254e1dde1903069dc3`. The evidence manifest also records this source identity. Its source tree is `610ce6eb2f6d25f3729501134c02ad29a58e408a`, executable SHA-256 `541fa19919f0033fe680ec15fcf1bc93570e00f05fe2af1ca09a49672e878c98`, and library SHA-256 `945f62ccc91a5f4ba45dd606f20bd80ee4787501c478042cd1f3f1fffbf1bbe4`. The rejection commit is `13e5a4325ea98c360376d5fce9c7bcdcffd34e15`. Before this report, its complete tree equalled the baseline tree `8a11ba823b3b8ce77bc385e137421479ad0e6185`.

Both builds use ordinary default-feature release binaries with Rust 1.98.1 and bundled SQLite 3.53.2. Products and the sibling Fake Provider were bound to logical CPUs 2, 4, 6 and 10 on the existing Linux i9-14900K/ext4/NVMe host. Timing processes run without profiling hooks or strace. The same executable is observed externally in the diagnostic runs.

Pi Durable is pinned to `1cedd32724abfcb0915f76cc61b6827e2c16dbad` and measured pi-rust to `4056176bf4867f913099b101fd65b773895c99ba`. Pi demonstrates internal integer identities and latest-only live documents. The inspected pi-rust append path uses `write_all` and `flush`, with batching and reclaimable progress. Their prior results remain archival measurements under their respective persistence and completion contracts.

## Ownership and physical indexes

Canonical block descriptors used the existing immutable Ledger position and block ordinal. MessageId resolves through the native `message_ledger` index at the boundary. The candidate block row retained its numeric foreign key, finite BLOB and rowid leaf layout.

| Consumer or structure | Result |
|---|---|
| `UNIQUE(message_id, block_index)` | Remove `sqlite_autoindex_observation_block_headers_2` |
| `observation_block_kind_idx` | Remove the MessageId/kind/block access tree |
| Numeric primary key | Retain `sqlite_autoindex_observation_block_headers_1` |
| `observation_block_ledger_kind_idx` | Retain bounded kind selection by numeric owner |
| Block capture/upsert | Resolve native position in the INSERT and conflict on position/block |
| Tool argument publication rebind | Resolve MessageId in a scalar subquery, then read/update the numeric key |
| Block pagination and preview | Use position/block and position/kind/block ranges |

The insert resolved identity inside its current transaction. The obsolete MessageId foreign key also disappeared. Publication rebind gained a native identity lookup for each block read/update. Block-page reads gained one native point lookup to check the owner header. The public numeric-locator entry point used a read transaction for its complete validation and page selection.

The candidate SQL returned only inline native identities of at most 256 UTF-8 bytes. Larger identities use the numeric locator, with metadata-only length/type checks. Reading a numeric page avoids loading those original strings or message bodies. The deterministic fixture includes an 80,000-byte original identity and invalidates message_json after capture while retaining readable block descriptors and bytes.

Owner checks cover the header's numeric position, bounded inline identity, locator, block ordinals and content-reference conversation/owner. Missing or conflicting observations produce an error. Tests cover rollback after an intervening source-deletion trigger, incomplete block pages, malformed owner bindings, long identities and bounded forward/reverse query plans.

Schema 103 rejects schema 102 before decoding or rewriting it. The gate fixture reconstructs the exact prior block-header DDL and both old indexes, then checks byte-identical refusal through both open APIs. External ContentRefs retain their exact published shapes.

## Structural comparison

The existing diagnostic phase socket and authenticated WAL replay supplied the four structural products. Each hot interval had 32 turns, 160 write commits, 256 native events, 32 request bindings and 224 Trace definition rows. Commit roles were inbound acceptance, adoption, attempt/turn start, request start and final settlement. Each product used WAL/NORMAL, 2 KiB pages, a 1,000-frame auto-checkpoint and the existing 8 KiB stream segments.

| Setup turns | WAL frames, baseline → candidate | WAL KiB/turn | Main-file KiB/turn | WAL + main KiB/turn | Main-file syncs | Closed database pages |
|---|---|---|---|---|---|---|
| 8 | 7350 → 7180 | 464.77 → 454.02 | 110.44 → 108.69 | 575.20 → 562.70 | 7 → 7 | 1267 → 1253 |
| 32 | 7897 → 7704 | 499.35 → 487.15 | 142.50 → 139.56 | 641.85 → 626.71 | 8 → 8 | 1836 → 1815 |
| 64 | 8137 → 7920 | 514.53 → 500.81 | 154.06 → 133.88 | 668.59 → 634.68 | 8 → 7 | 2611 → 2584 |
| 96 | 8683 → 8397 | 549.05 → 530.97 | 169.19 → 187.31 | 718.24 → 718.28 | 8 → 9 | 3410 → 3377 |

The 96-history candidate wrote fewer WAL frames while its main-file checkpoint traffic increased. Total hot page payload was effectively flat at that history. The hot main-file sync count moved from eight to nine. Policy settings were identical. Closed database space and cumulative writes have separate measurement endpoints.

An earlier correctness prototype also retained the four diagnostic histories. Its 96-history WAL plus main traffic rose from 718.24 to 719.04 KiB/turn. The final candidate was rebuilt after correcting a lint and bounding comparisons of long identities. Its fresh diagnostics and the earlier prototype ledger are both archived.

The 64-history baseline had 66 block-header table pages, eight MessageId unique-index pages, eight MessageId/kind pages, three numeric kind pages and one numeric primary-index page. The candidate had 55 table pages, three numeric kind pages and one numeric primary page. Total ownership dropped from 86 to 59 pages. The header BLOB tables had zero overflow pages in this sample and retained their rowid layout.

The syscall decoder authenticated WAL checksums and attributed pages at each committing image. dbstat describes retained layout. Process write_bytes, WAL/main payloads, wchar, cancelled_write_bytes and sync calls remain separate quantities. The earlier process-versus-syscall residual was carried forward.

## Ordinary fake-provider results

Four independent pairs per original scenario ran in balanced alternating order. Both builds passed the complete native-history, reply, observer, final-close, reopen and exact request checks. The following percentiles are calculated inside each process, then shown as their four-run median and range in milliseconds. Burst and paced SSE each have one turn per process, so P50 equals P95. Paired changes are medians of within-pair changes.

| Scenario | Baseline P50 [range] | Candidate P50 [range] | Baseline P95 [range] | Candidate P95 [range] | Paired P50 / P95 |
|---|---|---|---|---|---|
| 8-turn history | 11.95 [7.19–13.10] | 11.76 [9.92–13.38] | 17.85 [14.02–18.07] | 17.11 [13.51–17.45] | -5.49% / -4.43% |
| 64-turn history, initial four pairs | 8.01 [7.24–14.03] | 10.99 [8.17–14.14] | 12.67 [10.55–19.70] | 17.70 [12.45–19.81] | +13.35% / +27.09% |
| 32 KiB burst | 30.64 [30.46–38.19] | 31.04 [30.70–31.59] | 30.64 [30.46–38.19] | 31.04 [30.70–31.59] | +0.99% / +0.99% |
| 128-chunk paced SSE | 1546.71 [1537.94–1555.68] | 1548.65 [1535.23–1552.57] | 1546.71 [1537.94–1555.68] | 1548.65 [1535.23–1552.57] | +0.16% / +0.16% |

The CPU/write window spans the measured turns. Close writes span pre-initialization through final writer close. PSS is the maximum sampled hot-window value per process. Closed main-file space subtracts each opened-empty layout. Values are side medians except for the paired write change.

| Scenario | First consumed ms | CPU ms/turn | Hot write_bytes KiB/turn | Paired writes | Close write_bytes MiB | Sampled hot PSS MiB | Main KiB above empty |
|---|---|---|---|---|---|---|---|
| 8-turn history | 11.93 → 11.61 | 15.16 → 14.84 | 607.6 → 595.6 | -1.99% | 24.34 → 23.82 | 89.11 → 89.51 | 2036 → 2012 |
| 64-turn history, initial four pairs | 7.95 → 10.97 | 11.41 → 14.22 | 744.8 → 698.8 | -6.09% | 64.33 → 62.25 | 93.95 → 94.31 | 4722 → 4672 |
| 32 KiB burst | 30.63 → 31.00 | 30.00 → 25.00 | 592.0 → 576.0 | -2.70% | 2.15 → 2.11 | 81.62 → 81.82 | 250 → 246 |
| 128-chunk paced SSE | 127.06 → 127.22 | 120.00 → 120.00 | 788.0 → 772.0 | -2.03% | 2.29 → 2.25 | 81.42 → 81.27 | 180 → 176 |

### Fixed long-history uncertainty check

The initial 64-history cell had four slower P50 pairs and a +17.13% paired CPU median. Before any additional run, the chat recorded a fixed four-pair extension for this uncertainty. The binaries, fixture, affinity and observer remained the same. All eight pairs are included below. Each original observation remains in the first matrix and the full CSV.

| Metric | Baseline eight-run median [range] | Candidate eight-run median [range] | Median paired change | All eight paired changes, percent |
|---|---|---|---|---|
| p50_ms | 9.40 [7.24–14.03] | 9.47 [7.44–14.14] | +1.35% | +0.79, +0.56, +62.76, +25.91, -46.06, +1.91, -28.77, +11.51 |
| p95_ms | 14.89 [10.55–21.52] | 16.64 [12.45–19.81] | +0.28% | -6.34, -14.19, +83.08, +60.52, -32.60, +6.90, -24.12, +17.94 |
| first_consumed_ms | 9.34 [7.23–14.01] | 9.46 [7.42–14.13] | +1.88% | +0.84, +1.84, +62.91, +25.91, -44.92, +1.92, -28.79, +11.46 |
| cpu_ms_per_turn | 12.97 [10.00–17.50] | 13.12 [10.31–18.44] | +7.32% | +9.26, -7.69, +50.00, +25.00, -41.07, +5.56, -16.98, +9.09 |
| process_write_kib_per_turn | 744.81 [739.75–746.62] | 697.88 [695.88–700.75] | -6.11% | -5.97, -5.93, -6.21, -6.31, -6.42, -5.98, -6.01, -6.32 |
| process_write_mib_through_close | 64.33 [64.24–64.44] | 62.24 [62.13–62.37] | -3.27% | -3.01, -3.47, -3.10, -3.31, -3.23, -3.33, -3.41, -3.15 |
| max_sampled_hot_pss_mib | 93.81 [93.36–94.77] | 94.31 [93.70–95.41] | +0.63% | +0.44, +0.32, +0.66, -0.02, +0.96, +0.61, +0.69, +1.10 |

Six of eight long-history P50 pairs were slower. CPU rose in five pairs, with a paired median of +7.32%, while hot process writes fell by 6.11% in every pair. CPU variation remains unattributed. The additional native lookup is confirmed by the SQL plans. Given this overhead and the adverse observations, the candidate fails the iteration's retention rule.

The original schema-102 acceptance report's adverse short-history +4.96% P50 and +7.54% P95 remain part of the baseline evidence. This report preserves the new initial long-history +13.35% P50 and +27.09% P95 alongside the full eight-pair extension. Burst and SSE paired settlement medians changed by +0.99% and +0.16%. Single-turn CPU readings have a 10 ms tick resolution.

## Decisions on further mechanisms

The 64-history image contained 579 canonical payload/block/message frontier keys and 5,466 bytes of textual ledger IDs. Integer keys could reduce that logical portion. There is no controlled page-write benefit for that change in this iteration. It would also require coherent binary-source and metadata encoding changes. The representation was left at schema 102.

The candidate's remaining event traffic was led by kind, primary Trace kind and attempt indexes. Of 256 hot events, 224 had an attempt ID and 160 had a turn ID. Attempt IDs were 52 bytes, with mean event-kind length 18.43 bytes for those rows. Existing native start coordinates do not supply one complete authority for standalone, inherited and interrupted scope queries. A compact attempt index would require additional owner resolution or a mapping structure. This experiment retained the native scope indexes.

Page size stayed at 2 KiB and checkpoint policy stayed at 1,000 frames. The rowid BLOB layout was retained throughout the candidate. Fixed-context estimator serialization remains a separately scoped CPU investigation. This storage experiment added no accounting cache.

## Correctness and final state

The candidate's final source passed 1,917 focused all-feature library tests with three ignored and Clippy across all targets/features with warnings denied. Its external contracts, durable, process and provider targets passed 31, 151, 68 and 188 tests, with six provider tests ignored. The real integration binaries were rebuilt for schema 103 before those external tests. The final library run followed the mechanical lint and bounded-long-identity corrections. Formatting and whitespace checks passed.

Commands used `cargo build --locked --bins --all-features`, `cargo test --locked --all-features --lib --test contracts --test durable --test process --test provider --no-run` and the corresponding test binaries. The focused library selection covered SQLite, runtime client, agent execution, scripted agent, App Server, durable boundaries and archive suites. The Clippy command was `cargo clippy --locked --all-targets --all-features -- -D warnings`.

The 40 ordinary product samples and 936 exact request comparisons all passed. Every measured turn produced consumed content. Both builds retained final journal/canonical/request totals of 320/81/40, 768/193/96, 8/3/1 and 8/3/1 by scenario. Slow-subscriber, old-cut, rollback, process-death, archive, cancellation-before-start, failed-start and independent observation failure checks are covered in the deterministic suite. The candidate's gate tests explicitly rejected schema 102 and tested indexed forward/reverse plans.

The rejection restores the complete production and test tree to `1bce12684`. PR #485 remains Draft. Its exact final report HEAD triggers the existing FULL profile. The baseline FULL run had macOS and browser failures. Final remote state is available in [PR checks](https://github.com/Caismis/rustX/pull/485/checks).

The [40-product CSV](evidence/canonical-coordinates-20261109/all-per-run.csv), [raw evidence archive](evidence/canonical-coordinates-20261109/evidence.tar.xz) and [manifest](evidence/canonical-coordinates-20261109/evidence.tar.manifest.json) preserve all initial and supplemental pairs, identities, authenticated ledgers, operation counts and key inventories. Full syscall traces remain in the local measurement directory.
