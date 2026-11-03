# Native latency repair and the 128-session Pi memory sample

Two demonstrated causes of native latency and repeated CPU work are repaired without reverting the preceding memory/write changes. In a fresh four-triad Rust-only cohort, the immediately previous version versus the fixed version measures **23.06→10.78 ms** settled median, **17.81→12.19 ms CPU/turn**, **685.13→641.13 KiB process writes/turn**, and **780.87→766.87 MiB anonymous PSS** at 128 retained sessions with exactly eight completed turns each. Paired median changes are respectively **−50.48%, −31.59%, −6.46%** for the active width-one metrics; these are paired ratios, not ratios of the separately aggregated medians.

The one explicitly authorized Pi 128×8 **memory-only** product measures **310.46 MiB anonymous PSS / 373.75 MiB RSS**, compared with fixed rustX **766.87 / 830.15 MiB**: **2.47× / 2.22×**. This matches session/history counts, provider response text and retained native subscriptions, with important library-versus-AppServer capability limits below. No Pi latency/CPU/write benchmark, Pi rebuild or DeepSeek execution was performed.

[All values and adverse cells](2026-11-03-native-regression-tables.md) · [Raw evidence and portable offline audit](evidence/native-regression-20261103/README.md) · [Previous memory/write phase](2026-11-03-memory-writes.md).

## Why the earlier width-one cell rose 71.2%

The earlier report's +71.235% is the median of **two** paired changes, **+128.295% / +14.174%**, dominated by one candidate product near 50 ms rather than its baseline near 22 ms. The earlier clustered-header cohort had the opposite direction. This was an adverse observation worth investigating, not evidence that computation consistently increased by 71.2%. There is no archived packet trace for that earlier cell, so an exact retrospective decomposition is unavailable.

The native WebSocket acceptance path never set `TCP_NODELAY`. The controlled experiment keeps the actual saved O3 native ELF unchanged and uses a small accepted-socket-only diagnostic interposer to set that option. Each product runs 128×8 plus 32 width-one rounds and the standard history/slow-reader/cancellation checks; all 1,059 requests and native invariants pass. Both enabled cases record eight successful accepted-FD option settings.

| Saved executable / accepted socket | Settled median ms | Native CPU ms/turn |
| --- | ---: | ---: |
| Previous / unchanged | 20.25 | 18.44 |
| Previous / TCP_NODELAY | **10.88** | **13.75** |
| Legacy / TCP_NODELAY | 11.42 | 13.44 |
| Legacy / unchanged | 13.59 | 16.56 |

This isolates a large avoidable transport component without changing SQLite or observation functionality. TCP_NODELAY disables Nagle's buffering of small TCP segments. That mechanism is consistent with discrete small-message latency bands; these measurements do not isolate an exact kernel timer or prove all of the historical +71.2% came from it. [Linux TCP documentation](https://man7.org/linux/man-pages/man7/tcp.7.html).

Production now sets `socket.set_nodelay(true)?` on the accepted WebSocket branch before replay wrapping and the handshake. The bulk HTTP upload/download branches are unchanged. Message order, physical acceptance, replay ownership and durability boundaries remain.

## The actual CPU leader removed

A separate 250-Hz native-only profile rotates 256 active attempts across all 128 sessions. Histories stay near 8–10 turns instead of growing one session to 264. Legacy and previous CPU in this diagnostic are **15.51 / 15.27 ms per window turn**; the old +13.177% CPU direction is not stable under this fixed-history experiment either.

The actionable repeated work is Session catalog activity publication, already present before the last memory change. Every human prompt cloned all 128 persisted session records, reread `catalog.json`, decoded a full `serde_json::Value` tree and then the typed document, validated it, rebuilt a temporary observation index, pretty-encoded the full catalog and atomically replaced the file. Approximately 18–20% of the original profile's cumulative samples contain the catalog activity stack.

The repair uses the existing `imbl::OrdMap` dependency for persistent shallow document clones, updates ownership generations only for changed values, and shares one immutable published byte/generation record. Every commit still acquires the product ownership guard and stable-directory file lock, confines the path and **reads the current file bytes**. Only exact equality with previously validated bytes permits generation reuse. Changed bytes take normal strict decode/validation; missing/corrupt files and stale generations are not hidden by a timestamp or generation-only cache. Cold/changed reads inspect the schema envelope without building a second catalog-sized DOM. Encoding is compact; successful commit transfers ownership of its encoded bytes into the published snapshot.

The snapshot is published only after the existing atomic commit outcome, including the existing committed-but-directory-durability-uncertain outcome. Failure before commit keeps the previous generation and byte proof. File synchronization, generation assignment, ownership changes, invalidations and stale-write refusal remain. There is no new stored schema, compatibility path, migration or independent invalidation protocol.

| Diagnostic CPU sample group, overlapping | Legacy | Previous | Fixed |
| --- | ---: | ---: | ---: |
| All samples | 994 | 985 | 747 |
| Catalog activity | 193 | 178 | **35** |
| Catalog activity containing JSON | 140 | 126 | **11** |
| SQLite | 173 | 151 | 177 |

Each sample represents 4 ms; group rows are inclusive and overlap. Raw binary, collapsed stack and text totals agree. This supports the identified computation reduction, not an exclusive phase partition or proof that SQLite became slower. Final closed catalog bytes fall from approximately **152,272 to 109,335**. Compact JSON reduces repeatedly rewritten logical file bytes by about 28%; it is not itself a WAL or device-write counter.

## Fresh ordinary Rust comparison

The three immutable O3/default/glibc executables are legacy `1039cc44d` (schema93, 4-KiB pages), previous `b0ac8ed4f` (schema94, 2-KiB pages) and fixed `e1d4a40dc` (same schema94/page layout). Four balanced triads run legacy/previous/fixed, fixed/previous/legacy, previous/legacy/fixed, fixed/legacy/previous. These are **12 new ordinary native products**, not a mixture with profiler/interposer runs.

All use 128 independent sessions/workspaces, four Tokio workers, allowed CPUs 2/8, an explicit 128-resident limit, eight deterministic seed turns, then 16 rounds at widths 1/4/8. Production's resident default remains eight. Each product verifies **1,235 requests**, exact per-session HTTP input history and persisted messages, cancellation with partial-output absence, Trace/history, paused receiver, detach/reload, integrity/FKs and consistent closed-image hashes. Driver/provider are sibling processes; CPU, `write_bytes`, RSS and anonymous PSS belong to the native server PID.

| Width | Previous→fixed settled median ms | CPU ms/turn | Process writes KiB/turn | Paired wall / CPU / write changes |
| --- | --- | --- | --- | --- |
| 1 | 23.06→**10.78** | 17.81→**12.19** | 685.13→**641.13** | −50.48% / −31.59% / −6.46% |
| 4 | 40.71→**29.75** | 13.98→**11.95** | 710.78→**667.06** | −29.15% / −14.04% / −6.17% |
| 8 | 64.48→**57.57** | 13.36→**11.80** | 737.27→**692.89** | −10.72% / −12.28% / −6.01% |

At width one, legacy→fixed paired wall/CPU/write changes are **−62.40% / −28.29% / −29.54%**. The legacy medians in this cohort are 36.73 ms / 16.56 ms / 910.38 KiB, showing why fresh same-cohort comparisons are necessary rather than subtracting old medians.

The broader empty-server→all-unloaded window, including creation, all seeding and active probes, history, slow-reader and cancellation work, measures median CPU **24,420→21,285 ms** and writes **892.33→831.95 MiB** previous→fixed. This is total lifecycle work, not per-turn chat CPU, and helps check that the hot-window improvement was not merely deferred past settlement.

| 128-session memory endpoint | Legacy anon / RSS MiB | Previous anon / RSS MiB | Fixed anon / RSS MiB |
| --- | --- | --- | --- |
| Empty attached | 336.67 / 390.87 | 329.43 / 383.43 | **323.42 / 377.87** |
| Exactly eight turns each | 925.61 / 989.29 | 780.87 / 843.92 | **766.87 / 830.15** |
| All unloaded after later probes | 1016.55 / 1077.26 | 849.82 / 910.10 | **840.21 / 900.83** |
| Reload/unload cycle two | 1016.56 / 1077.43 | 849.84 / 910.21 | **840.26 / 901.07** |

These are medians of four process endpoints, not continuous peaks or a leak/capacity guarantee. Anonymous memory remains high after unload because process allocator retention and live ownership are different measures; that existing distinction is not changed by this repair. The preceding shared capabilities, 560-byte event enum, bounded replay semantics, 2-KiB SQLite pages and rowid block headers are all retained. No page-cache or statement-cache capacity is reduced. The previous phase's approximately 6–7% higher final used-page growth remains a retained storage tradeoff; this round does not rerun or update that WAL diagnosis.

All adverse samples remain. In triad three, fixed width one measures **66.86 ms** median versus previous **21.54 ms** (**+210.43%**) and CPU **+22.22%**; its acceptance times are also elevated. This product is included in every aggregate. At width four that triad has median **+0.13%** and unchanged CPU. Pair zero at width eight has fixed median **+0.56%** and CPU **+5.42%** versus previous, and one fixed width-eight product has roughly **1.34 s P95/P99**. The preserved counters do not identify the cause of these individual shared-host pauses; they cannot be assigned to the removed TCP or catalog work merely from their duration. Across four triads the medians improve, but the tails do not establish a P99 guarantee. Each width has only 15/60/120 nonheld turns per process. Latency excludes the deliberately held first round; resource windows include it and orchestration gaps. The host is shared. Compiler-quiet preflights and the retained two-second monitor are not exclusive-host proof; the monitor starts after the first legacy product and records no compiler-positive observation in its sampled interval. No outlier or entire product is discarded.

## The authorized Pi 128×8 memory sample

The user explicitly allowed only one Pi 128-session memory supplement. It uses unchanged Pi source **1cedd32724abfcb0915f76cc61b6827e2c16dbad**, the previously built cached runtime, **295 tracked and 967 derived file hashes** matching the saved prior build, and ordinary Node **24.19.0**. There is no rebuild, renewed typecheck or forced GC. Earlier Pi build/typecheck limitations remain as documented in the request-hotpath report.

The process holds 128 independent workspaces, Harness instances and SQLite files, a shared model registry and 128 native `watchEvents` subscriptions. It receives the same 1,024 deterministic seed HTTP requests/response text and waits for each completion plus observer run-end. The eight-turn endpoint is captured while all 128 remain retained; `/proc` FD evidence confirms exactly 128 conversation databases open. No per-turn time, CPU or write benchmark is collected.

| Endpoint | Pi anon PSS MiB | Pi RSS MiB | Open databases |
| --- | ---: | ---: | ---: |
| After imports | 64.93 | 118.99 | 0 |
| Empty 128 | 103.49 | 163.16 | 128 |
| All subscribed, empty | 105.48 | 165.14 | 128 |
| Eight turns each | **310.46** | **373.75** | **128** |
| Closed | 311.25 | 370.66 | 0 |
| Reopen checks completed | 314.82 | 374.22 | 0 |

At the seed point Node reports approximately 109.53 MiB heap used and 217.62 MiB heap capacity, but V8 heap-used is not whole-process memory or a forced-GC live-object estimate. Reopen verification happens **after** the authoritative seed endpoint: all 128 API entry lists, eight assistant replies and exact user markers are saved, the instances close, then each reopens and reproduces its canonical entry hash. Independent raw audit checks 128 consistent SQLite backups, integrity/FKs, API/database record equality, eight done submissions each and all 1,024 full HTTP histories.

Thus fixed rustX's measured anonymous PSS is **456.40 MiB higher**, or **2.47×**, and RSS is **456.40 MiB higher**, or **2.22×**, for this count/history-matched workload. Rust values aggregate four products, Pi is **one sample**. Pi's native library/watch subscription does not include rustX's WebSocket AppServer, Inspector, frozen historical request/domain proofs or equivalent replay/failure-isolation services; tool rosters are four versus seven. This is a useful native-process memory endpoint comparison, not full feature equivalence. The whole remaining gap cannot be assigned to any one cache or feature from these totals, and old single-session values are not multiplied by 128.

## Correctness, failed attempts and provenance

New catalog regression coverage verifies changed current bytes are not hidden, pre-rename faults preserve the old published generation/bytes, retry succeeds, and reopen reproduces state. **3,251 unique tests pass, three existing ignored**, across 17 relevant command groups; formatting and release/all-target/all-feature Clippy with warnings denied pass. The full local-runtime subset contributes 878 passes/two ignored, with eight artifact-lineage tests deduplicated across overlapping filters. All **972 source/test/Cargo inputs** match checks, fresh ordinary build and production. Correctness builds use separate root-O0/all-feature artifacts.

The expanded local-runtime run exposed a pre-existing test that treated private compact Trace BLOB bytes as UTF-8 JSON. Only its setup assertion is repaired to use bounded public typed Trace reads; the original NULL-corruption and selected child-span independence assertions remain. A later parallel test run hit the existing 10-second agent flood timeout under shared-host load after completing tool work. The final 585-test agent group and local-runtime subset pass serially; no deadline or production behavior is weakened. Eight already-green checks are reused only after all 972 input hashes match, with raw log equality independently checked. Both unsuccessful attempts and coordinator failures remain archived.

Compiler preflight failures and an initial missing-profile filename used with pprof are retained separately. Corrected symbolization uses the actual `active.cpu.0` file, checks its existence and agrees with raw sample totals. Original rotating-driver identities are frozen by reversing only the later preflight assertion-to-wait change and proving the exact original digest; measurement logic, identities and data are not edited.

Production **e1d4a40dc7876d79ca0b3240064b560659b7d593**; native SHA256 **e28644d77b236ccc955b90f65753e6914d7ec1f336b0af6b9fe80ae2afdb6c0f**; conformance SHA256 **6ad1e625481759aef3ddda09d8dfad3c114beb46e90854788dd32651933b1f42**. Twenty accepted native products comprise 12 ordinary, four CPU diagnostics and four controlled TCP products, plus eight non-chat stream/large/tool/cancel functional products. They are not 28 independent timing repetitions.

The successful portable replay authenticates all **9,050 selected raw members**, including **2,560 Rust and 128 Pi consistent database images**, and recomputes native counters, request histories, closed images, functional checks, CPU denominators, Pi memory/database proof, source maps, host observations, summaries and all tables. It executes no product, compiler, git, symbolizer or network and relies on recorded prearchive byte proofs for deliberately excluded executables/libraries and Pi runtime. Full repository CI/browser E2E and a main merge are not claimed. Existing main conflicts remain and PR485 stays draft.
