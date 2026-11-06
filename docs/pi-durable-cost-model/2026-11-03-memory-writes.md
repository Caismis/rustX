# Retained observation memory and actual SQLite writes

The final Rust-only 128-session ×8-turn comparison reduces anonymous PSS **924.64→780.19 MiB (-15.62%, 144.45 MiB)**. Immutable capability sharing and smaller replay-event storage address a measured retained-memory leader. Compact SQLite pages reduce actual write amplification; the final layout still increases the 32-turn used-page growth by approximately 6–7%, a retained storage tradeoff. **Pi and DeepSeek were not executed.**

[All values](2026-11-03-memory-writes-tables.md) · [Raw evidence and offline replay](evidence/memory-writes-20261103/README.md).

## Final ordinary native 128-session comparison

Two balanced pairs, before→after then after→before, reproduce the prior roughly 925-MiB baseline. Seed values are captured after exactly eight completed turns per session. Numbers below are medians of two process endpoints, not a continuous peak.

| Phase | Before anon MiB | After anon MiB | Before RSS MiB | After RSS MiB |
| --- | --- | --- | --- | --- |
| empty_server | 14.24 | 14.25 | 46.46 | 46.19 |
| metadata_only | 31.53 | 31.46 | 74.26 | 73.59 |
| empty_attached_128 | 337.46 | 329.63 | 392.10 | 383.46 |
| seeded_history_8 | 924.64 | 780.19 | 988.30 | 843.29 |
| all_unloaded | 1016.04 | 848.58 | 1077.14 | 908.77 |
| reload_unload_cycle_2 | 1016.06 | 847.63 | 1077.28 | 907.88 |

Seeded before values [925.25, 924.0390625]; after [778.55078125, 781.83203125]. All 128 actors remain loaded at the seed point. The production resident default remains eight; this harness explicitly permits 128.

| Active width | Before/after settled median ms | CPU ms/turn | Process writes KiB/turn | Paired median wall change | Paired median write change |
| --- | --- | --- | --- | --- | --- |
| 1 | 20.89 / 36.53 | 16.56 / 18.75 | 909.88 / 685.38 | +71.235% | -24.674% |
| 4 | 55.11 / 39.85 | 14.84 / 14.61 | 937.50 / 710.97 | -27.670% | -24.162% |
| 8 | 118.51 / 71.08 | 14.84 / 14.34 | 977.06 / 737.06 | -33.860% | -24.563% |

The final native width1 cell has a retained adverse result: paired median wall **+71.235%** and CPU **+13.177%**. Its two wall changes are +128.295%/+14.174%; the earlier clustered-header cohort has the opposite width1 wall direction. No candidate or outlier is removed, and these small shared-host cohorts do not establish a stable latency effect. This change is retained for measured live-memory and write reductions, not a claim of faster every-path execution.

Settled latency excludes each deliberately held first wave; resource windows include all waves and orchestration gaps. Only 15/60/120 nonheld turns per width per process: P95/P99 and throughput remain descriptive. Shared-host activity and all paired reversals are retained in the full tables.

## Measured SQLite and replay ownership

| SQLite category | Before per connection KiB | After per connection KiB | Before 128 MiB | After 128 MiB |
| --- | --- | --- | --- | --- |
| cache | 945.15 | 820.65 | 118.14 | 102.58 |
| statements | 722.65 | 722.05 | 90.33 | 90.26 |
| schema | 213.55 | 213.47 | 26.69 | 26.68 |

These are approximate SQLite API counters in private root-O0 probes, at sequence 64 in each of 128 connections. The subtotal is not a partition of ordinary glibc PSS. Plan counts before/after: [210.0, 210.0].

| Replay-release probe | Untouched sessions | Median released MiB/session | Min/max MiB | Median events | Inline storage KiB/session |
| --- | --- | --- | --- | --- | --- |
| sql-replay-serial-before | 95 | 2.67 | 2.56 / 2.73 | 104 | 147.00 |
| sql-fields-128-refined | 95 | 1.70 | 1.60 / 1.76 | 105 | 71.00 |

Global allocator differences can overlap asynchronous teardown and shared owners; they are approximate release observations. Do not multiply their medians into an exact 925-MiB ownership partition or add them to a different allocator/PSS denominator. The independently measured ordinary seed reduction above is the actual endpoint result.

| Sampled requested MiB, inclusive groups overlap | Before | After |
| --- | --- | --- |
| sqlite_malloc | 236.71 | 231.96 |
| sqlite_page_cache | 127.41 | 117.05 |
| checkpoint_materialization | 120.16 | 21.47 |
| publication_encoding | 222.12 | 127.97 |
| replay_deque_growth | 19.15 | 9.00 |
| schema_compilation | 63.09 | 67.72 |

The checkpoint group covers the complete materialization closure, not only capabilities. Publication encoding includes allocations also counted below materialization. These rows corroborate source-level ownership and are not exclusive phase totals.

## Actual writes and final storage tradeoff

| Observer/history | Warm wall paired change | CPU paired change | Process-write paired change |
| --- | --- | --- | --- |
| no-live-subscriber/8 | -1.654% | -1.786% | -27.679% |
| no-live-subscriber/64 | +1.412% | +1.613% | -25.391% |
| live-subscriber/8 | -4.637% | +1.429% | -27.717% |
| live-subscriber/64 | -3.993% | -3.721% | -25.278% |

Four balanced pairs per cell; 1,088 exact paired requests. The table uses medians of paired changes, not ratios of independent medians. No universal latency/CPU improvement is claimed.

| History | Side | Page bytes | WAL frames | WAL frame bytes | Commits | Net used-page growth |
| --- | --- | --- | --- | --- | --- | --- |
| 8 | baseline | 4096 | 5629 | 23191480 | 160 | 1470464 |
| 8 | complete | 2048 | 7000 | 14504000 | 160 | 1564672 |
| 64 | baseline | 4096 | 6380 | 26285600 | 160 | 1474560 |
| 64 | complete | 2048 | 7810 | 16182320 | 160 | 1576960 |

Frame count can increase while total frame bytes fall. The parser uses each image’s actual page size plus the 24-byte frame header, checks every header/payload/offset and commit, and also replays unchanged historical 4-KiB traces as a golden check.

- 8 historical turns: final consistent used-page growth **1,470,464→1,564,672 bytes (+6.407%)** per 32 turns.
- 64 historical turns: final consistent used-page growth **1,474,560→1,576,960 bytes (+6.944%)** per 32 turns.

The initial 2-KiB clustered-header cohort grew used pages by about 14%. In the short diagnostic its header overflow-page growth was 64; the final rowid-header diagnostic has zero header overflow pages at both endpoints. Removing those spills mitigates, but does not eliminate, the main-image growth penalty. Additional rowid-key indexes and different packing remain. This is an explicit write-amplification/memory versus final-page-density tradeoff, without deleting logical facts. Native closed-image totals and two-session ×512-history CPU/write/file results are all in the full tables.

## What was retained and why

Checkpoint capability capture previously retrieved a prepared `CapabilityView`, deep-cloned it with `Cow::into_owned()`, then wrapped the copy in a new `Arc`. Every retained checkpoint could therefore own another copy of the same unchanged tools/configuration. The prepared cache now owns the `Arc<CapabilityView>` itself. Checkpoints share it until its immutable owner/generation changes. A regression checks pointer sharing, changed-owner isolation and the byte stability of an already issued checkpoint. This removes repeated ownership rather than reducing the number of historical checkpoints.

The event enum's three largest inline payloads are boxed: transcript/optional Trace in `ReadDomainsUpdated`, and the descriptor in `MessageDescriptorCommitted`. The O3 `RuntimeClientEvent` layout falls **1,168→560 bytes**; its layout regression limits future growth. Wire JSON/schema, replay ordering/cursors, retained event/byte limits and public event semantics remain. Rust source payload types intentionally change; no compatibility wrapper is added.

Small content encodings no longer immediately reserve a 64-KiB scratch allocation. The active buffer grows geometrically from 256 bytes up to the existing 64-KiB bound; two-chunk buffering and persisted bytes are unchanged. The existing small-value test also checks the actual capacity.

New SQLite databases use 2-KiB pages before WAL initialization. This reduces bytes per dirty-page write without combining transaction boundaries or dropping facts, indexes or commits. The first complete trial exposed a real adverse effect: `observation_block_headers` used a clustered index representation, whose medium BLOB payloads spilled to overflow pages at the smaller page size. Its physical table is now an ordinary rowid table, preserving every logical column, PK/unique/FK constraint and query index. The schema owner checks this physical shape and a regression rejects the former clustered shape. Private schema **94** refuses earlier schemas; no migration/fallback is introduced, following repository policy.

SQLite distinguishes table-leaf and index-page payload thresholds and notes that large rows can make WITHOUT ROWID less efficient. This is why only the measured header table changes representation. [File format](https://www.sqlite.org/fileformat2.html#b_tree_pages), [WITHOUT ROWID guidance](https://www.sqlite.org/withoutrowid.html).

## Interpreting SQLite and replay ownership

Direct diagnostic probes use `sqlite3_db_status` for page-cache, schema and prepared-statement bytes, plus statement-plan counts. These are SQLite's approximate connection categories, not an exact partition of OS PSS. [Counter definitions](https://www.sqlite.org/c3ref/c_dbstatus_options.html).

The largest retained statement plans are ordinary event/request/message/observation writes and current Trace/Statistics reads. The pinned rusqlite 0.40.2 implementation clears bindings when a statement returns to its cache; these counters do not establish another cache of bound event BLOBs. The bounded 256-plan capacity is preserved instead of introducing repeated prepare work.

Replay release is measured with the native child's allocator live-byte counter, clearing the retained ring during teardown. The comparison uses 95 untouched eight-history sessions, indices 32–126; active-session prefixes, transferred observer ownership and the slow final session are excluded from this particular ownership estimate. Every raw release value and outlier remains. Even sequential detach can overlap asynchronous native cleanup, so these deltas are approximate medians/ranges, not additive per-object byte accounting. Sharing can also leave an owner alive after a replay reference is released. Replay wire-byte counters are admission accounting; the ring stores typed events rather than another retained serialized buffer.

The complete sampled heap export independently checks the allocation stacks. It uses installed gperftools 2.15 `GetHeapSample`, a 64-KiB sampling period and a tiny source-archived signal adapter in the native child. Raw record counts/bytes sum exactly to the exported headers; independent Poisson correction agrees with symbolized collapsed weights within integer rounding. This avoids the older fixed 1-MiB full-tracking exporter limit. Weighted requested sizes, allocator size-class-charged totals and ordinary glibc anonymous PSS have separate denominators. Inclusive stack groups overlap and must not be added.

The seeded 128-session increase is largely live allocation. After unload, the sampled live allocation count falls substantially while the allocator retains free pages; high post-unload OS PSS therefore has a different explanation from the seeded live-memory increase. Neither flat reload cycles nor these finite diagnostics proves the absence of every leak.

## Measurement boundaries and retained adverse trials

The native harness uses 128 distinct blank workspaces/sessions, four Tokio workers, two allowed CPUs, an explicitly configured 128-resident limit and deterministic local responses. The production resident default remains eight. The host is not exclusive. An unrelated worktree also compiled during the overall verification period. The final runner waits for a compiler-quiet window before each top-level product, records those waits, and retains low-rate host compiler observations; this is not an exclusive-host latency experiment. Driver/provider are siblings; OS resource counters are for the native server PID. Two balanced ordinary before/after pairs are descriptive, not a production capacity/SLA claim. The seed point is captured after exactly eight turns per session, before the subsequent 1/4/8-wide activity/history/cancellation probes. Each formal native run verifies all 1,235 provider requests.

Native validation checks exact per-session wire history and persisted message bodies/counts, cancellation with partial-output absence, database integrity/FKs, paused receiver, Trace/history reads, detach, repeated reload and consistent image hashes. Held first waves are excluded from latency quantiles; CPU/write windows include all waves and orchestration gaps. Replacement-allocator/root-O0 diagnostics are never pooled into ordinary timing or memory.

The initial compact-page/schema93 cohort, its approximately 14% larger used-page growth, CPU/latency reversals and full data remain separately archived. It is not pooled with the final rowid-header/schema94 cohort. Earlier failed check attempts expose the schema-owner rule and stale literal expectations subsequently repaired; the corrected full suite is authoritative. Waiting-coordinator terminations and recoveries are recorded. A standalone layout probe initially used the cache directory’s older default compiler; it was rerun with the repository-pinned 1.98.1 compiler. Formal executable/source identities were unchanged. The older full-tracking eight-session diagnostic completed its native/provider workload but missed its last requested signal and exported incomplete bucket coverage; it is retained as exploratory evidence, excluded from the complete sampled-profile comparison.

Ordinary single-session before/after cells use four balanced pairs each, 8/64 history and absent/live subscriber. Live means that benchmark's runtime subscriber, not App Server/WebUI transport. All paired directions and mean/outlier values are in the tables. Separate two-session ×512-history probes check growth without fitting a complexity law. Logical column growth, WAL frame bytes, process `write_bytes`, consistent main-image used pages and post-exit files are separate measures; none is device-level SSD writes.

## Validation and source identity

Final production **b0ac8ed4f06ef15c6da52438eee06a4e00fd7737**, fresh O3/default-feature conformance SHA256 **0e88bfd1c7ea5fb792e6d1aa13bde2e1d05659f89342dde6388d89a9be522827**, native **bf1e7b5f6a64d1266aa731402de72bd570de5aced253230bc5e1a4426f7e5a4e**. Baseline is the saved schema 93/O3 production 1039cc44d (documentation 1c947513f): conformance 6274155c… and native 2add0f8a…. All 971 source/test/Cargo inputs match checks, build and current production source. Correctness artifacts use separate root-O0/all-feature builds.

**2,382 tests pass, zero fail, one existing ignored**; formatting and release/all-target/all-feature Clippy with warnings denied pass. Benchmark helpers pass 48 Python and 24 Node tests, which do not execute Pi/DeepSeek. Source/build maps for the initial and intermediate candidates remain separate. Both 32-product storage cohorts, 16 non-chat functional products, 23 native products with closed images, and 9 unique complete sampled heap exports are independently audited. This is not a count of independent same-scenario timing samples.

The portable offline replay authenticates every selected archive member and rechecks raw counters/WAL, full per-session wire histories, native message contents/cancellation absence, SQLite integrity/FKs/image identities, sampled-profile denominators, ownership probes, check/build source maps, host observations and generated tables. It runs no product, compiler, git, symbolizer or network. It relies on stored build/executable identity proofs, since binaries/libraries are deliberately excluded. Existing main conflicts remain; this does not claim full repository CI/browser E2E or an updated Pi ratio.
