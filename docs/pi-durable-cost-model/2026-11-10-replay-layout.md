# Compact Replay event slots with preserved payload ownership

## Scope and decision

**Accept with notes for the representation-only memory reduction; Revise before merge for the PR.** The simple layout reduces the partial reachable-event census and populated capacity medians, while eight-to-32 growth falls in all four clean pairs. Empty-session PSS rises, ordinary short-history CPU/latency is adverse, and shared-machine follow-ups do not establish its cause. Keep these limits explicit; there is no general speedup, solved CPU-regression or flattened write-growth claim. Do not merge.

This iteration retains the preceding bound Ledger coordinates and ContentRef ownership work. The only new production mechanism is local enum layout: six unusually large event fields move behind Box so every small replay event no longer pays their maximum inline size. No additional cache, authority, retention mode or persistence layout is introduced. Pi Durable and pi-rust remain read-only source references. Neither they nor DeepSeek were rebuilt or run.

The ordinary baseline is bb132f76595f34409fa4b1d3bc1095ee22f71e53, Rust tree e193107b32cf0faafe6b418c1c4c28964aaf8256. Diagnostic-only commit 80f32ca4fe5872c2c206d2915487578ebee347c1 adds status/variant attribution; its Rust tree is 6b6718ee51f514f2c0c6721ea514466f01133f6e and no default production behavior changes. The candidate is 86bf61a3050dbf85818386d7242d141c59785439, Rust tree 42fe0c56decf66012cd4320784ba5e9967124c40.

Candidate default release O3/debug0 conformance SHA-256: `1adb4bd2885829bcb5aa1d6ff400b88c90b07e835385d931960365bfd99f9689`; CLI: `3d7e0d94429d1cb5e60dc1f787d8f3195c5e23b7056d3daa784bd6fb22604c64`. A fresh root library, conformance and CLI were authenticated; all seven harness inputs equal the baseline. Diagnostic binaries and their feature identities remain separate in the evidence.

## Reference principles and chosen owner

Pi-rust at 4056176bf4867f913099b101fd65b773895c99ba shares large streaming payloads in crates/rpi-agent/src/events.rs. Its frames use separately reclaimable progress, not a second transcript. RustX already shares final events through Arc and retains historical replay; adding another Arc to each payload would not improve that owner. Box only moves six large variant fields out of the common slot. Pi Durable at 1cedd32724abfcb0915f76cc61b6827e2c16dbad uses latest-only LiveDoc and retires generation/tool presentation at settlement. Copying its retirement rule would discard rustX replay/cut guarantees and is not part of this change.

ToolExecutionSettled.result, AgentStatusComposed.status, ToolCallAssembled.call and the three ContextCompaction event context fields are boxed. The measured event size falls from 552 to 400 bytes. JobUpdated remains inline at 400 bytes; boxing frequent progress merely to lower the ceiling a further 16 bytes is not justified. ProjectionTransition metadata remains inline. Public Rust field types intentionally change in this pre-1.0 codebase; generated protocol schema and fixtures remain byte-identical. SQL, WAL/NORMAL, page size, checkpoint policy, commit roles, replay limits, frozen inputs and public JSON do not change.

## Partial retained-owner census

One diagnostic product per revision, 128 sessions each, 32 turns, phases at 8/16/32. Census includes capacities, unique owners, Arc headers and every new payload Box. It does not partition ordinary PSS or include all private caches, nested row buffers, allocator rounding or SQLite. Diagnostic compiler overlap is recorded and excludes all CPU/wall/PSS claims from these products. All 256 backup hashes, schema, integrity, FKs and exact ordered messages were independently checked. Every counted host disappears after unload.

| Depth | Known requested MiB before | After | Event envelope/vector MiB before | After |
|---|---:|---:|---:|---:|
| 8 | 53.514 | 48.955 | 16.773 | 12.212 |
| 16 | 110.320 | 101.179 | 33.524 | 24.387 |
| 32 | 224.011 | 205.832 | 67.047 | 48.785 |

The six boxed variants are absent from this native text fixture, so their added Box bytes are zero here. Tool/context/status correctness tests cover the affected variants and a diagnostic unit test checks payload counting once. Read-domain publication counts vary slightly with scheduling; all native facts and requests remain exact. Do not treat the partial-census change as whole-process MiB.

An initially plausible status-row ownership rewrite is rejected for this workload: the census contains no AgentStatusComposed events or retained status rows. A 64-row maximum and 30-minute time-refresh interval do not prove rows exist or cause this fixture's growth. No new prepared-status wrapper or cache was added.

## Ordinary adverse results and measurement limits

The original four cells have four balanced independent pairs each. Ordered-content/options and observer semantics pass, including a separate tool/cancel functional pair. No rustc is observed during the original matrix, but the original monitor did not identify clippy-driver/cargo/linkers; it is not a claim of an idle machine. Raw data remain unchanged.

| Original ordinary pairs | P50 paired median | CPU | Hot process writes |
|---|---:|---:|---:|
| 8 setup turns | +42.72% | +25.48% | -0.01% |
| 64 setup turns | -3.46% | +1.92% | +0.01% |
| Cold 32 KiB burst | -7.03% | -12.50% | 0% |
| Paced SSE | +1.35% | -0.51% | 0% |

All four short-history P50/CPU changes are positive. All original negatives are retained; neither favorable long/burst values nor the lower object-size count cancels that observation. Long-history P95 includes a 1,450 ms candidate outlier. Endpoint CPU has 10 ms granularity and includes driver work. Do not infer a fixed runtime regression percentage or claim a general speedup.

A predeclared four-pair short-history follow-up constrains both unchanged binaries/provider to CPUs 8 and 10. Clippy activity was observed during this follow-up but missed by its original rustc-only monitor. Its first two pairs remain strongly adverse/mixed in magnitude; the last two have approximately 10 ms CPU/turn on both sides. This does not prove the original regression fixed or prove scheduling alone caused it. One reverse-order 1000-Hz signal profile per side has 361/332 hot-window samples; cumulative stacks overlap and are not exclusive time percentages. Profiles preserve GNU allocation and are diagnostics only.

Selected cumulative sample numerators are sqlite3_step 86/361→81/332, publication encode 55/361→43/332, checkpoint capture 35/361→26/332 and benchmark process_sample 31/361→34/332. These stacks overlap. No new dominant path is identified at this resolution; the lower candidate counts do not prove speedup or disprove the original adverse CPU window. Plain-text census has zero new payload Boxes, so new rare-variant Box allocation is not an observed operation in that fixture; code generation and allocator/layout effects are not ruled out.

The first native capacity pair is retained but excluded from headline PSS because the candidate overlaps external rustc. The sequence stopped before pair 1; completed products were not rerun. Before collecting further capacity products, one replacement pair was declared and guards expanded to rustc, clippy-driver, cargo and linkers. Pairs 1-4 supply the intended four balanced clean capacity pairs if their audit passes. All ten products and negative results remain archived.

## Native capacity

Four clean balanced pairs (IDs 1-4) use the ordinary GNU allocator and server-PID-only sampled anonymous PSS. All ten products including the excluded contaminated pair are retained. All 1,280 backup hashes, exact ordered content/counts, schema 106, integrity and foreign keys pass independent validation. Expanded compiler overlap counts are zero in the eight headline products; this is not a claim that every other host process was idle.

| Anonymous PSS MiB | Before side median | After | Median within-pair change |
|---|---:|---:|---:|
| 128 subscribed empty | 338.148 | 341.891 | +0.550% |
| 128 × 8 | 656.945 | 654.576 | -0.501% |
| 128 × 16 | 827.236 | 820.727 | -0.731% |
| 128 × 32 | 1123.346 | 1106.398 | -1.599% |
| Same-process 8-to-32 growth | 465.178 | 454.006 | -2.482% |

Growth falls in all four pairs, but populated endpoints fall in three of four. Pair 3 has higher empty and populated endpoints; empty PSS rises in every pair (+0.27% to +3.39%). The layout does not solve empty-session baseline. Seed CPU is 13.770→13.781 ms/turn (paired +0.087%) and writes 675.153→675.157 KiB/turn (+0.003%). This workload does not reproduce the original short-conformance CPU magnitude; it does not erase that original result. PSS after unload remains allocator residency, not proof of reachable replay owners. Census after unload is separately empty.

## Native activity and deep-history supplements

One pair per active pattern uses 128×8 setup, four active sessions and 256 active turns. All 512 consistent backup hashes, exact messages/order/counts, schema, integrity and FKs pass. Slow-consumer/resync, browse, cancellation and close/reload checks pass. These are supplements, not four independent runs or universal regression-absence evidence.

| Pattern | CPU ms/turn before → after | P50 change | P95 change | Process write change |
|---|---:|---:|---:|---:|
| Fixed four | 14.414 → 15.117 | +14.74% | +24.49% | -0.05% |
| Rotating four | 19.805 → 15.273 | -32.52% | -24.67% | +2.48% |

The fixed candidate active window contains six expanded compiler-overlap monitor samples; its adverse CPU/P50/P95 are retained but cannot isolate the layout effect. Rotating active windows have no observed compiler, but rchar rises 33.83% and read calls 141.14%, including native idle-retirement work. A single pair with different idle/scheduling behavior does not demonstrate a CPU fix or lower reads. Do not combine these percentages with the original conformance matrix.

One reverse-order 16×64 pair is 204.988→203.492 MiB sampled anonymous PSS. All 32 backups pass independent exact-content/integrity/FK/hash checks; compiler overlap is zero in both products. It is a supplementary endpoint, not a precise deep-capacity percentage. Across census, capacity, activity and deep products, 2,080 backups were independently validated.

## Correctness

Fmt, all-target/all-feature Clippy with warnings denied, fresh all-feature CLI/supervisor prerequisites and 1,672 distinct focused tests pass, three ignored. Overlapping filters produce 1,840 successful executions. Layout <=400, payload census, old cuts, slow subscribers, cancellation, request-start failure, frozen/retry input, atomic settlement, terminal ordering and independent observation failure groups pass. Protocol generation yields no schema/fixture diff. These are local focused checks, not full-project or completed remote CI.

Initial compilation/test-constructor failures, the diagnostic too-many-lines warning and a protocol-description-only diff are retained. The internal layout comment was changed from Rustdoc to an ordinary comment, then the final correctness pipeline reran successfully. No public documentation/schema addition is hidden.

## Remaining write cost and bounded next plan

Reuse the authenticated preceding storage ledger; the new layout changes no persisted representation. At 8/96 setup turns it attributes 548.42/680.25 KiB per hot turn of WAL+main syscall payload. The 131.83 KiB difference is 76.32 KiB WAL and 55.50 KiB checkpoint main-file output. It is not process write_bytes or NAND output, and neither topology rewrites all old history. Five semantic commits and 8 native events per turn remain.

| Group | WAL+main KiB/turn at 8 | At 96 | Increase |
|---|---:|---:|---:|
| Events and native/observation indexes | 191.30 | 225.73 | 34.43 |
| Canonical/transcript | 132.71 | 160.22 | 27.51 |
| Other B-trees/page 1 | 44.25 | 63.21 | 18.96 |
| Request manifest/Source | 52.78 | 69.44 | 16.66 |
| Content frontiers/layout | 50.06 | 64.76 | 14.69 |
| Request owner/snapshot/model | 53.22 | 65.63 | 12.40 |
| Trace catalog | 24.08 | 31.26 | 7.17 |

Next rank actual event-index key bytes/fan-out, then choose one native scope family using existing authenticated numeric coordinates. Keep all bounded historical/negative queries and external IDs; reject new joins or authority maps with little page benefit. content_frontiers is the largest single-table WAL source (682/777 frames in those endpoints); inspect repeated same-page updates and inline/overflow geometry before changing policy. The request Trace catalog is a bounded secondary candidate: 32 requests still write 224 definition identity rows despite shared schema bodies. Exact immutable ordered catalogs could own those rows, with per-request binding and independent availability evidence. Its entire low-history group is only about 4.4% of output, not a predicted net saving. Do not prioritize it over the verified larger groups without a simpler evidence-backed reduction.

For memory, continue unique-owner attribution of frontier/binding fragments rather than adding global interning or another cache. Old cuts, current-byte authentication and independent failure boundaries remain. Status-row sharing is not a measured priority. Do not shorten replay, reduce idle TTLs, change allocator, restore sparse Statistics, move terminal work beyond the endpoint or copy Pi's latest-only guarantee onto rustX.

No Pi, pi-rust or DeepSeek product was run. The source-HEAD remote snapshot has #485 open/Draft, currently conflicting with main; only the notification workflow is recorded successful. Required exact-final-HEAD correctness gates remain unknown. Main integration is a separately requested follow-up after these frozen pre-integration measurements. Do not wait for CI or merge the PR as part of this memory iteration. The broader history-growth objective remains active.

## Evidence retention

See [build identity](evidence/replay-layout-20261110/build-identity.json), [unchanged source/helpers](evidence/replay-layout-20261110/source-validation.json), [census](evidence/replay-layout-20261110/census-summary.json), [ordinary per-run CSV](evidence/replay-layout-20261110/ordinary-per-run.csv), [capacity](evidence/replay-layout-20261110/memory-summary.json), [activity](evidence/replay-layout-20261110/native-summary.json), [deep](evidence/replay-layout-20261110/deep-summary.json), [CPU profiles](evidence/replay-layout-20261110/cpu-profile-symbols.json) and [source-HEAD remote status](evidence/replay-layout-20261110/remote-source-head.json). The [preceding authenticated storage ledger](2026-11-10-block-coordinates.md) remains the write-cost source; it was not rerun or mislabelled as a new layout experiment.

The [portable archive](evidence/replay-layout-20261110/replay-layout-portable.tar.xz) preserves all raw measurements, request/process/per-turn records, failed/preflight-stopped and adverse products, monitors, helper/build identities, verifier outputs and backup hashes, with representative database images. The separate full archive preserves every one of the 2,080 verified native backups. ELF/dependency trees and private/live runtime roots are excluded. The [archive index](evidence/replay-layout-20261110/archive-index.json) and member manifests define exact scope; portable is not claimed to contain every database image.
