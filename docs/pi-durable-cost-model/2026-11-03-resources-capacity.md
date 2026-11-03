# Actual writes and native multi-session capacity

This round closes the bounded resource investigation following [request preparation](2026-11-03-request-hotpath.md). It adds a real App Server workload and tests a storage layout change against actual process counters and WAL pages. **It does not establish a large speedup, a memory leak, or production capacity. Pi and DeepSeek were not rerun.**

The retained production change is deliberately small: `observation_models` uses its compound primary key as its storage tree, removing its separate rowid/primary-key tree. The broader two-table trial was narrowed after it increased long-history writes. No new runtime cache, materialization protocol, persistence domain, or transaction boundary is introduced.

[All values](2026-11-03-resources-capacity-tables.md) · [Evidence and portable replay](evidence/resources-capacity-20261103/README.md).

## Storage decision: remove one tree, retain lifecycle locality

The initial schema-92 trial clustered both `lifecycle_state` and `observation_models`. Per 32 warm turns its diagnostic WAL frames changed **5,655→5,622** at 8 historical turns, but **6,384→6,409** at 64. All four long-history process-write pairs increased in both observer modes: paired medians **+0.465% / +0.375%**. These results are retained, not replaced by the later trial.

Endpoint page ownership identifies why the lifecycle assumption failed: in the long-history diagnostic its old data and primary-key trees accounted for 68+73 frames, versus 173 after clustering. Changing the storage order made terminal updates dirty more pages. The model group instead fell from 143 to 132 frames, counting its remaining unique/owner indexes. These are endpoint `dbstat` associations, not exclusive timing or proof that every frame delta has one cause.

The final schema-93 candidate restores lifecycle rowid locality and clusters **only models**:

| History | Before WAL frames | Final frames | Change | Write commits |
|---|---:|---:|---:|---:|
| 8 | 5,655 | 5,626 | −0.513% | 160→160 |
| 64 | 6,376 | 6,364 | −0.188% | 160→160 |

These are separate diagnostic products. They are not the ordinary process-write samples below. SQLite describes this representation as useful for small rows with noninteger primary keys, while large rows can behave worse; it is not a general instruction to cluster all tables. [SQLite guidance](https://www.sqlite.org/withoutrowid.html).

The final four ordinary pairs per cell give:

| Observer / history | Warm median wall change | Warm CPU change | Warm process-write change |
|---|---:|---:|---:|
| None / 8 | −1.305% | −5.704% | −0.351% |
| None / 64 | −1.682% | −3.571% | −0.103% |
| Live / 8 | −2.699% | 0% | −0.336% |
| Live / 64 | **+0.490%** | −2.740% | **+0.084%** |

Each cell reports the median of paired changes, not a ratio of independent medians. Long-history writes have reversals in both directions. One short before product had substantial wall outliers; mean wall values, all pairs, and the product remain in the tables. Four pairs do not provide a precise improvement estimate. Live means the existing benchmark subscriber, not App Server/WebUI transport.

**This is a modest structural simplification, not a material solution to the Pi write gap.** It removes a real maintained B-tree without adding ownership or invalidation state; that is why it is retained. The two-table experiment is finished, and no third speculative layout was added. `content_frontiers` was already `WITHOUT ROWID`. The Source-page proposal was rejected before building: earlier tests deliberately require rowid locality for large BLOBs.

All columns, uniqueness constraints, independent domain sources, query indexes, event rows, historical cuts, and commit boundaries remain. Schema verification rejects a shadow rowid model table. Private schema 93 rejects prior versions without a migration, following repository policy. Existing failure-isolation/current-byte/rollback tests remain in the full passing suite.

Implementation: [schema and verification](../../src/durable/sqlite.rs), [layout refusal test](../../src/durable/sqlite/compact_lookup_tests.rs).

## Capacity: actual native server, separate fixed and session costs

The new [driver](../../benchmarks/runtime/app-server-capacity.py) talks to the real authenticated WebSocket App Server. Its [provider adapter](../../benchmarks/runtime/app-server-capacity-provider.py) uses the existing deterministic provider implementation. Driver and provider are sibling processes; `/proc` counters and profiles belong to the **native server PID only**.

The bounded setup uses two allowed CPUs (2,8), four Tokio workers, 128 distinct blank workspaces/sessions, built-in tool definitions, and a local provider. It explicitly configures 128 residents, 512 global attachments and a one-second idle grace. **The production resident default stays eight.** There are no executing external tools, containers, remote model latency, or large real workspaces in this workload.

The harness respects the real API: one controller per session, at most 32 attachments per physical connection, six ordinary request credits, two control credits, and separate observation lanes. Four primary connections own sessions round-robin. Three additional subscriber connections temporarily own disjoint sessions; they are not duplicate observers on a single session. Earlier illegal ownership/lane attempts are retained as setup failures, excluded from performance results.

Final ordinary server anonymous PSS / RSS:

| Phase | Anon PSS MiB | RSS MiB |
|---|---:|---:|
| Empty server | 14.20 | 46.27 |
| 128 metadata-only sessions | 30.21 | 72.59 |
| First loaded empty session | 102.62 | 152.66 |
| Eight loaded empty sessions | 119.39 | 169.77 |
| 128 loaded empty sessions | 336.50 | 390.63 |
| 128 subscribed empty sessions | 336.76 | 392.01 |
| Eight completed turns per session | 925.43 | 988.50 |
| After activity/history, all actors unloaded | 1,019.50 | 1,080.19 |
| After three eight-session reload/unload cycles | 1,019.51 | 1,080.27 |

The first loaded session includes shared initialization; multiplying its cost by 128 would be misleading. Beyond the initial shared work, the empty-loaded 8→128 slope is approximately **1.81 MiB per additional session in this scenario**. This is not an exact heap object size or a bound for arbitrary history/workspaces. Eight turns per session add substantial memory, and subscription-only marginal memory here is small. The baseline and discarded two-table candidate show similar overall scales; the layout is not a memory optimization.

Each of the baseline/final 128-session ordinary runs completes 1,235 provider requests. The independent audit checks every request's full per-session marker history, exact native user/assistant contents and counts, cancelled-output absence, database integrity/FKs, consistent-image hashes, active-root counts, and detach/reload results. All ordinary final unload points have loaded/loading/unloading zero.

The final nonheld active windows give:

| Simultaneous attempts | Observations | Accepted median ms | Settled median ms | Settled P95 ms |
|---|---:|---:|---:|---:|
| 1 | 15 | 6.52 | 19.14 | 66.30 |
| 4 | 60 | 16.35 | 44.48 | 79.52 |
| 8 | 120 | 21.70 | 61.96 | 106.21 |

These are client-observed WebSocket timings, not isolated native queue time. Each width starts with one deliberately held provider wave to sample active memory; it is excluded from latency quantiles and burst service throughput. Remaining gates are released before requests. CPU/write counter windows include all rounds and orchestration gaps. Burst throughput excludes setup gaps and is not sustained production capacity. One process per scenario and 15–120 observations do not establish P99 or an SLA.

The workload also browses history, reads Trace during a held attempt, pauses one receiving controller during a 32-KiB/128-delta response, cancels another attempt, detaches, and reloads history three times. It verifies cancellation settles and partial cancelled output is absent. The slow-reader case does **not** demonstrate that every transport/application queue was saturated or prove an arbitrary slow-consumer memory bound.

## Unload: distinguish object release from OS memory retention

The high anonymous PSS after unload is real. Three ordinary reload/unload cycles remain approximately flat, but a flat sequence alone neither proves all allocations are freed nor excludes all leaks.

A separate bounded **eight-session** diagnostic replaces only the native child's allocator with the installed tcmalloc heap profiler. It uses the unchanged baseline native library, one seed turn per session, and 1/4/8 active attempts. Global live-allocation headers show **110,088,842 bytes during the held eight-attempt phase → 77,339,073 after all unload → 76,532,806 after three cycles**. This demonstrates allocation release in that bounded workload and is consistent with allocator high-water retention plus long-lived shared structures. It does not attribute all of the ordinary 128-session retained GiB or prove absence of every leak. Replacement-allocator RSS and timings are not ordinary measurements. The first 128-session heap calibration was too intrusive and was safely aborted; no conclusion uses it.

Among serialized buckets at unload, symbolized stacks through `jsonschema::compiler::compile` account for approximately 54.6 MiB. There is a critical tool limit: the installed package is **gperftools 2.15-3build1**, whose heap exporter has a fixed 1-MiB serialization buffer. At this point the global header reports 77,339,073 live bytes but serialized buckets represent only 68,640,233 (**88.75% coverage**). `pprof`'s 65.5-MiB total is the represented subset, not the whole heap; the schema stack's percentage must not be presented as a whole-heap percentage. [Exact 2.15 implementation](https://raw.githubusercontent.com/gperftools/gperftools/gperftools-2.15/src/heap-profiler.cc).

Raw global/represented totals, omissions, and coverage for all six heap dumps are audited. Signal dumps follow observed phases by approximately 10–20 ms and are not synchronous barriers. One diagnostic reload point actually has unloading=1, retained explicitly; the next two are zero. The harness now waits all three lifecycle counts on every reload cycle; a separate final 37-request smoke passes. The source used for the formal measurements remains archived separately.

## Longer native CPU sample

An independent final-server diagnostic profiles the last 8-wide window for 64 rounds, excluding seeds, using signal-controlled 250-Hz sampling only in the native child. Raw records, collapsed stacks and text output all agree on **1,758 samples** (the discarded candidate separately has 1,771). [Profiler controls](https://gperftools.github.io/gperftools/cpuprofile.html).

Final exclusive leaf classifications are approximately 20.4% serde-related, 15.3% allocation, 11.8% copying/comparison, 9.2% SQLite, and 43.3% other. SQLite appears somewhere on 391/1,758 stacks (22.2%); `sqlite3VdbeExec` cumulative output is 309 samples. These inclusive observations overlap; they cannot be added to leaf shares or interpreted as exclusive phases. Optimized inlining, generic type names and identical-code folding also limit attribution. The sample supports distributed serialization, allocation and database work, not a newly proven single removable dominant cost. No new runtime cache was justified by it.

## History growth and uniform file endpoints

Separate ordinary before/final probes seed **512 turns in each of two sessions** and verify all 1,075 requests. Final settled medians at widths 1/2 are 51.81/28.64 ms; native CPU is 18.13/19.06 ms per turn. Process writes are 1,234/1,189.25 KiB per turn, with mixed before/final directions. A single process at each point is a growth check, not a complexity fit or a claim of improved long-history throughput.

Consistent SQLite backups distinguish used pages from running file sizes:

- Final 32-turn storage diagnostics grow used main-image pages by **1,470,464 bytes** at both 8 and 64 history; before grows 1,474,560 / 1,470,464 bytes. No dramatic long-term storage improvement is shown.
- The final native 128-session workload ends with **141,029,376 used-page bytes**, versus 141,553,664 before, for the same request/content oracle. This includes many distinct session databases and is not a per-turn measure.
- Two 512-history sessions end at **52,232,192 used-page bytes** versus 52,252,672 before. The final 128-session images contain four free pages in total (main files 141,045,760 bytes); the two 512-history images contain six (main files 52,256,768 bytes). After graceful exit their WAL/SHM files are absent. Main-file totals agree with consistent allocated images; used-page totals subtract the free pages.

Running WAL footprint, process `write_bytes`, logical column bytes, and final used-page growth are different metrics. None is device-level SSD writes. The older roughly 38× mixed file endpoint is not reused as a compact database comparison. No new Pi file-size/write ratio is calculated.

## Prior full-cycle memory was partly benchmark verification

The previous committed 32 fceaf/Pi records are re-read without executing either product. Counter-only excerpts retain their original member hashes and archive provenance. In long-history Rust no-subscriber records, phase medians move from approximately **117–119 MiB** near warm completion to **172.70 MiB** after frozen-request verification and **202.54 MiB** after reopen; live reaches **207.47 MiB** after reopen. All four long-history peaks occur at reopen, sometimes tied with cleanup.

This locates the earlier full-cycle peak outside ordinary warm execution. It does not explain the new native 128-session history/residency cost, or turn endpoint sampling into a continuous peak measurement. Previous Pi ratios remain the previous report's results; they are not recomputed against this batch.

## Checks and outcome

Production candidate **1039cc44deff7e13b8a8275ab12e8b2b09cb7ad2** uses fresh O3/default-feature ordinary binaries. Correctness builds use separate root-O0/all-feature artifacts. All **971** checked source/test/Cargo inputs match the final ordinary build and current production source. Final validation passes **2,379 tests, one existing ignored**, formatting and all-target/all-feature release Clippy with warnings denied; benchmark tests pass **48 Python + 24 Node**. Both ordinary storage cohorts independently verify 1,088 request comparisons each. Native products and their closed images are separately audited, never combined into a count of independent timing samples.

The baseline conformance executable is the saved fceaf O3 build. The baseline native executable is an explicit O3 `main.rs` relink against that saved, source-matched default-feature library and dependencies, not a fresh library build. All command, compiler, source, binary and fixture identities are archived. Final native SHA-256 is `2add0f8a10f8c29238b24124be4f10cfde9cd9ee34d7b6a97fa63f0ba19876ea`; final conformance is `6274155c7721d0ebb2a55324c9d6c22d8d15fac44e067320408c198e41725425`.

This phase is complete: one redundant model tree is removed, the harmful lifecycle change is withdrawn, real multi-session resource growth is measured, prior memory peaks are located, and a longer native profile is available. Actual writes remain substantial and history/resident memory grows materially; the results do not justify a broad cache or persistence rewrite for a promised large speedup.
