# PR #485: continued validation and DeepSeek Harness reference, 2026-10-27

> Historical D/E round. The [joint runtime report](2026-10-27-joint-major-runtime.md) identifies the latest delivered source and fresh Pi/DeepSeek comparisons; the [F write/compute report](2026-10-27-write-compute-reduction.md) retains the intermediate evidence.

**Delivered source is D, and overall Pi parity is still not achieved.** Against the previous retained B, median paired D/B ratios imply reductions of **15.9%** in no-live short-small turn wall and **9.6%** in complete CPU; the corresponding live short-large reductions are **3.3%** and **8.9%**. These are targeted three-pair results with adverse cases, not a general speedup. A fresh comparison of delivered D against pinned Pi still gives warm short-turn wall medians of **7.70–11.93×**, turn CPU **4.48–9.22×**, complete self CPU **1.98–5.43×**, and process-attributed writes **8.37–9.30×**.

The request-scalar trial E reduced writes but was **rejected and reverted**: dense explicit inspection became consistently slower, and no-live short-small wall and per-turn self CPU also regressed in every runtime pair. DeepSeek Harness is now a third real native SDK reference, with all 36 rotating products verified. Its idle boundary does not await the final persistence flush, so the report does not rank it as an equal-durability replacement for Pi or rustX.

This continuation contains **300 accepted formal products**: 72 D/B, 72 rejected E/D, 72 D/Pi, 48 B/D append diagnostics, and 36 three-engine products. The separate Reader diagnostic contains 7,168 reads. Each experiment keeps its own cohort and contract. Results below are newly measured; no historical ratios are multiplied. The [earlier B/C report](2026-10-27-performance.md) remains historical evidence.

## Source and measurement identity

| Artifact | Source revision | Ordinary optimized executable SHA-256 |
| --- | --- | --- |
| Previous retained B | `cb6e026b4ac84842703dfc6adbaec43d700bff24` | `f80f3b7ba0f2be6035c7459c87cf04e69ec596e64ccba0d72d43ddce7d8ebdb3` |
| Delivered D | `e8d6380fa6ea0724b752970473d093c629cb8a63` | `8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233` |
| Rejected E | `a39f343b54a46c8e5527dad88842cb8a6d46691b` | `19c52cb8b539abfc0a259d6493ad08b65df00a1e4cc782284cdf562ffa866cf7` |
| Pi Durable SQLite | `90d863f7c1183b965f099855715d6e88c7b8a52c` | Pinned checkout verified in each product |
| DeepSeek Harness SDK | `687f282e8625c6b8d11ff58211a90524be2e4317` | Source launch; frozen loader, lockfile, native addon and adapter hashes |

Revert `6e1ca189abfa9879fe87d2e4ecd053530434a491` restores **all tracked files** to D; `git diff --exit-code e8d6380fa HEAD` passed before documentation edits. Delivered `src` tree is `d6d193e07b16db0ab9b1ce646a254a1c1674c80e`, and schema remains 76. The final cohort used that restored harness revision and the preserved ordinary O3 D binary, avoiding rebuild ambiguity. The [build identity](evidence/continuation-20261027/d-build-identity.json) records fresh root artifacts, compiler, commands and identical six-file runtime harness hashes. The [final plan](evidence/continuation-20261027/final-plan.json) binds source/binary identities before execution and records every command, failure state and timestamp.

The final product cohorts ran serially from 00:30:05 to 00:34:32 UTC on October 27, followed by independent audits. No task-owned build, test, profile or compression ran during formal product measurement. Host: Linux 6.8, Intel i9-14900K, ext4, Rust 1.98.1, Node 24.19.0, 100 process CPU ticks/s; exact details are in [host.json](evidence/continuation-20261027/host.json). Unrelated machine activity cannot be ruled out by these records.

## Delivered D versus pinned Pi Durable

All cells are **median within-pair D/Pi ratios**, three alternating pairs per case and subscriber mode; lower favors D. Complete counters run since process exec through cleanup and include startup and verification. The short cases measure 32 turns after 8/64 history turns. The other cases are single first-request workloads, with two provider requests for the tool case. Both use the same sibling HTTP/SSE provider, fixed model-facing time and independent history/output checks. Rust/Pi retain their distinct seven/four native tools, so full cross-engine request equality is false.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live | short-small | 8.1807 | 4.4783 | 1.9789 | 8.3728 | 1.1508 |
| no-live | short-large | 7.7021 | 4.8500 | 3.8286 | 9.3044 | 1.2710 |
| no-live | stream | 0.9209 | 0.2500 | 0.2411 | 5.0182 | 0.8812 |
| no-live | large | 0.5007 | 0.4545 | 0.2712 | 6.0988 | 1.0838 |
| no-live | tool | 0.6252 | 0.5385 | 0.3279 | 6.0000 | 0.9467 |
| no-live | cancel | 0.4417 | 0.4000 | 0.2667 | 6.0252 | 0.9412 |
| live | short-small | 11.8607 | 7.7407 | 3.0600 | 8.3869 | 1.2035 |
| live | short-large | 11.9295 | 9.2174 | 5.4315 | 9.2993 | 1.3065 |
| live | stream | 0.9212 | 0.3231 | 0.2870 | 5.0182 | 0.8608 |
| live | large | 0.5156 | 0.5000 | 0.2833 | 6.0988 | 1.1027 |
| live | tool | 0.6295 | 0.5714 | 0.3279 | 6.0000 | 0.9643 |
| live | cancel | 0.4583 | 0.4167 | 0.3000 | 6.0252 | 0.9426 |

For absolute scale, these are **separate per-engine medians, D / Pi**, not the arithmetic used to obtain paired ratios:

| Mode | Case | Turn wall ms | Turn CPU ms | Complete writes MiB |
| --- | --- | ---: | ---: | ---: |
| no-live | short-small | 29.675 / 3.691 | 32.188 / 7.500 | 74.309 / 8.875 |
| no-live | short-large | 28.765 / 3.788 | 30.625 / 6.250 | 193.320 / 20.777 |
| live | short-small | 50.350 / 4.245 | 65.312 / 8.438 | 74.434 / 8.875 |
| live | short-large | 50.935 / 4.260 | 64.375 / 7.188 | 193.215 / 20.777 |

The live short-large wall pairs include a **23.10×** unfavorable observation (D 91.947 ms versus Pi 3.981 ms), retained without exclusion. Favorable cold stream/large/tool/cancel wall results do not establish warm or overall parity; those workloads still incur about 5–6× Pi process writes. The [raw recomputation](evidence/continuation-20261027/pi-recomputed.json) contains all absolute values, pair ranges and 90% log-t intervals. Three pairs are exploratory; the [comparison policy](comparison-policy.md) requires seven and its uncertainty criterion for a parity claim.

## Why the overall gap remains

D removes two bounded overheads: temporary publication-encoding buffers and repeated readiness transactions for the same installed batch. It does not remove most of the request capture, historical inspection or live-observation work. D/B writes remain essentially unchanged, which is consistent with the limited scope, not evidence of a storage-volume breakthrough.

rustX still constructs and validates the frozen request scalar each turn, derives navigable field geometry/previews, hashes authenticated pages, and writes root/frontier metadata. Stable model settings and tool schemas are part of that scalar even when unchanged. Its historical message nodes already reuse immutable native identity, so this is **not** a claim that the whole text history is recopied each turn. Live publication additionally maintains bounded checkpoints/read-cut metadata, validates wire budgets and refreshes transcript/process views. No-live mode retains enough publication state for later readers. These functions are not all part of the Pi or DeepSeek reference surfaces.

Pi already uses WAL/NORMAL, statement reuse and serialized transactions, throttles partial commits and reuses immutable context ranges with appended-suffix scanning. rustX already uses the same WAL/NORMAL setting; reducing durability further is not the demonstrated missing fix. Its additional observation structures and write volume remain. Historical B CPU samples located Source indexing, structural publication and live read-cut work, but they are not fresh D per-function attribution and cannot apportion the current gap in milliseconds.

The DeepSeek source provides a more concrete next design direction: within a stable request series it records header/context changes and folds appended events incrementally. Initial, resume and new-series boundaries can still append a header even when the configuration is unchanged. It still compares tool schemas, clones/freezes requests and copies message arrays; it is not constant-cost assembly. rustX could share immutable configuration/tool fragments while binding each historical request to its exact version. That would need to preserve configuration changes, retry/compaction, aliases, raw byte ranges and corruption checks. This round did not implement or claim a measured gain from that proposal. The rejected E trial shows why moving work from capture to inspection alone is insufficient. [Pinned source analysis and limits](evidence/continuation-20261027/deepseek-cost-notes.txt).

## Additional native DeepSeek Harness reference

The full official SDK profile, through its supported source launch, runs the real agent loop, native 25-tool configuration, HTTP/SSE provider, checkpoint policy, projection cache, compressed JSONL log and native session resume. The shipped `sdk-source` patch disables the unavailable generated Typert loader, as in the official source-launch resolver. Three Latin-order rotations cover four supported cases. All **36 products and 12 Rust/Pi comparisons** passed; each engine made 414 measured-lifecycle provider requests including seeding. DeepSeek additionally passed 12 native resume verification requests, for 426 total reference requests. Every closed native log and the appended resume history is checked; all concatenated Zstd frames are decoded. Tool and mid-turn cancellation are unsupported by this adapter and are not counted as successes.

**These are native API completion observations, not equal-durability comparisons.** Rust/Pi use their existing live-subscriber settlement contract. DeepSeek `session/wait` observes idle and notifications but does not await final flush/fsync; SDK notifications carry committed events/status rather than live partial text. Prefix checkpoints are awaited before a model request, while turn-end flush is asynchronous. Shutdown attempts to flush, but its `allSettled` disposal can suppress failure: exit zero and a readable resumed log are not a positive sync-success or power-loss certificate.

The table shows each engine's median **[full three-repetition range]**, with no cross-engine speedup ratio. Wall values are the three repetitions' mean measured-turn durations; their range is not a distribution of individual turns. Writes are sampled **per measured turn**, unlike the complete-process write column in the Pi table. PSS is the maximum of sampled turn observations for each product, not a process peak. Rust/Pi CPU counts self time; DeepSeek aggregates sampled CLI/descendant self time plus waited-child time (these samples contained only the CLI PID). DeepSeek's Python controller is outside CLI counters, while Rust/Pi include their in-process driver; source TypeScript loading affects SDK startup. These boundaries preclude a full-lifecycle ranking. [Adapter, setup and contract documentation](../../benchmarks/runtime/deepseek-reference.md), [strict independent audit](evidence/continuation-20261027/three-engine-audit.json).

| Case | Engine | API completion ms | CPU ms/turn | Writes MiB/turn | Sampled anon PSS MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| short-small | rustX D | 45.244 [37.516, 49.404] | 59.688 [49.062, 64.375] | 1.855 [1.852, 1.866] | 110.527 [110.203, 111.707] |
| short-small | Pi Durable | 4.028 [3.790, 4.109] | 8.125 [7.188, 8.125] | 0.214 [0.214, 0.214] | 106.613 [106.176, 107.531] |
| short-small | DeepSeek SDK | 15.377 [14.136, 15.418] | 17.812 [16.562, 17.812] | 0.021 [0.021, 0.021] | 245.668 [239.801, 246.938] |
| short-large | rustX D | 49.690 [43.024, 54.333] | 64.688 [56.562, 70.625] | 2.153 [2.148, 2.158] | 128.582 [128.246, 129.219] |
| short-large | Pi Durable | 3.960 [3.940, 3.973] | 6.562 [6.250, 7.188] | 0.214 [0.214, 0.214] | 126.887 [120.059, 129.098] |
| short-large | DeepSeek SDK | 15.159 [14.851, 15.581] | 17.188 [16.875, 17.812] | 0.020 [0.020, 0.020] | 260.047 [259.098, 265.277] |
| large | rustX D | 43.349 [42.650, 45.864] | 50.000 [50.000, 50.000] | 1.605 [1.605, 1.605] | 85.844 [85.410, 88.094] |
| large | Pi Durable | 88.801 [87.513, 92.118] | 120.000 [110.000, 120.000] | 0.281 [0.281, 0.281] | 78.016 [77.656, 79.465] |
| large | DeepSeek SDK | 164.197 [161.494, 174.913] | 190.000 [190.000, 200.000] | 0.031 [0.027, 0.031] | 196.461 [196.141, 209.262] |
| stream | rustX D | 1600.793 [1579.323, 1605.763] | 180.000 [170.000, 190.000] | 1.922 [1.922, 1.922] | 88.672 [88.547, 88.789] |
| stream | Pi Durable | 1696.841 [1622.923, 1770.958] | 580.000 [460.000, 670.000] | 0.492 [0.477, 0.527] | 102.113 [101.812, 103.723] |
| stream | DeepSeek SDK | 1634.067 [1625.581, 1641.018] | 330.000 [320.000, 340.000] | 0.027 [0.027, 0.027] | 208.090 [207.453, 218.434] |

Full native prompts/context and tool schemas intentionally differ. Rust/Pi preserve their existing core request oracle; the DeepSeek oracle independently authors ordered benchmark history/output expectations and separately checks a reviewed native-envelope capture. It does not pretend the three native requests are identical. Close/exit `wait4`, resource counters and resume work remain separate raw evidence. The independent numeric audit rechecks arithmetic, identities and recorded semantic-oracle structure; it does not reimplement the whole semantic oracle or prove crash durability.

## Final append diagnostics: D against B

The 48 products cover three pairs at histories 0, 64, 512 and 2,048, with 32 appends per product. Reopened means a new SQLite connection for each measured append, not a cold OS page cache. Each cell below is a paired D/B median **[all-pair range]**. Complete writes are equal within every pair. [Independent raw append audit](evidence/continuation-20261027/append-audit.json).

| Connection | History | Append wall | Append/reopen phase CPU | Complete CPU | Lifecycle wall | Sampled anon PSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| warm | 0 | 0.889 [0.654, 1.034] | 0.667 [0.500, 1.000] | 0.750 [0.400, 1.000] | 0.845 [0.682, 1.082] | 1.001 [0.999, 1.002] |
| warm | 64 | 0.991 [0.975, 0.994] | 1.000 [0.667, 1.000] | 1.000 [0.875, 1.125] | 0.981 [0.967, 1.070] | 1.000 [1.000, 1.001] |
| warm | 512 | 0.929 [0.907, 1.003] | 1.000 [1.000, 2.000] | 0.977 [0.977, 1.000] | 0.985 [0.962, 0.995] | 1.000 [0.999, 1.001] |
| warm | 2048 | 0.978 [0.976, 1.036] | 1.000 [0.800, 1.250] | 1.000 [0.962, 1.000] | 0.980 [0.951, 0.988] | 0.999 [0.999, 1.000] |
| cold | 0 | 0.975 [0.762, 0.982] | 1.000 [1.000, 1.059] | 1.000 [0.947, 1.000] | 0.976 [0.858, 0.989] | 1.000 [0.999, 1.000] |
| cold | 64 | 0.977 [0.974, 0.991] | 1.000 [0.938, 1.067] | 0.952 [0.875, 1.000] | 0.951 [0.930, 0.994] | 1.000 [0.999, 1.000] |
| cold | 512 | 0.999 [0.980, 1.043] | 1.000 [1.000, 1.059] | 0.983 [0.982, 1.000] | 0.984 [0.982, 1.008] | 1.000 [0.999, 1.000] |
| cold | 2048 | 0.962 [0.069, 0.967] | 0.909 [0.512, 0.952] | 0.982 [0.895, 0.987] | 0.990 [0.493, 0.992] | 1.001 [0.999, 1.001] |

Every append-wall median is below one, but the evidence does not establish a uniform or stable-tail improvement. Cold-0 separately bracketed per-operation CPU rises in all three pairs: B [0.625, 0.9375, 0] to D [1.875, 1.25, 0.3125] ms/append. The zero denominator makes the third ratio unavailable; a two-value median must not be reported as the three-pair result. Whole append/reopen-phase and complete CPU medians are both 1.0 there. At 100 Hz, 32 separately rounded short CPU windows are coarse. Cold-512 sampled total PSS/RSS rises in all pairs by roughly 1.3–2.6%, while anonymous PSS does not show the same consistent increase.

A favorable cold-2,048 outlier is preserved too: B mean append 40.230 ms versus D 2.759 ms. It cannot be presented as representative 93% acceleration. All slow/fast products, per-operation counters, lifecycle phases, missing/zero states and file-footprint checks remain in the audit; process-attributed writes are not device I/O.

## Retained D: exact publication accounting and atomic readiness release

D contains two production changes measured together. Its source head was `e8d6380fa6ea0724b752970473d093c629cb8a63`, with `src` tree `d6d193e07b16db0ab9b1ce646a254a1c1674c80e`. The earlier B comparison artifact had `src` tree `e3f821550dbe5ba8d54c269251b6e0ce5152b3b4`. Reverting E at `6e1ca189a` restored D's full tree. No timing benefit can be assigned separately to either of these two changes. The complete build/source/executable identities and unchanged benchmark-harness hashes are in the [D audit](evidence/continuation-20261027/d-retention-audit.json).

1. **Count publication wire bytes without retaining a temporary encoded buffer.** `PublicationDraft` retains the typed event and needs its exact structural JSON length for admission/accounting. It now serializes into a bounded counting writer. A positive safe-integer admission can avoid a repeated structural preflight; uncertain keys and wide native integers retain structural admission, and the final schema-aware serializer still visits and validates encoded values. Exact escaped lengths, native `u64` representation, inclusive budget boundaries, serializer errors, eviction and replacement behavior remain covered by differential tests against the buffered encoder. This removes the temporary byte-vector allocation/copy for publication accounting; it does not remove the final structural serialization or claim that downstream transport encoding disappears.
2. **Release one installed native observation batch in one readiness transaction.** After CPU-only projection installation, `mark_native_observation_ready` marks the batch's canonical transcript range, applicable transition transcript cursors, and direct Journal range in one SQLite `IMMEDIATE` transaction. It commits before waking the Journal observer once, outside both database and observer locks; runtime fanout readiness follows a successful release. Failure rolls back the whole release, invalid ranges remain rejected, and unrelated readiness holes remain closed. This combines readiness bookkeeping only; it does not merge the native execution commits or change SQLite durability pragmas. Tests cover atomic release, wake ordering, injected failure/rollback, reopen recovery, prior Journal holes, and transcript-only terminal release.

The D/B experiment contains all six cases in both subscriber modes, three alternating pairs per case/mode, and no selected reruns. The independent audit checked 72 product/controller records and reconstructed 1,692 HTTP request bodies, including full semantic equality of the 846 paired requests after the declared workspace-path/text normalization. It recomputed all eight recorded metrics, 288 paired ratios and 96 summaries. The table retains the five policy metrics; lifecycle wall, total PSS and RSS remain in the evidence. All values are **D/B paired-ratio median [minimum, maximum across all three pairs]**; values below 1 favor D. `short-large` is the 64-history/32-measured-turn workload, distinct from the single-turn `large` workload. [Recomputed evidence](evidence/continuation-20261027/d-retention-audit.json), [audit notes](evidence/continuation-20261027/d-retention-audit-notes.txt).

| Subscriber | Case | Mean turn wall | Turn self CPU/turn | Through-cleanup self CPU | Process writes | Sampled anonymous PSS |
|---|---|---:|---:|---:|---:|---:|
| No live | short-small | 0.8406 [0.8359, 0.8697] | 0.8571 [0.8250, 0.8980] | 0.9043 [0.8829, 0.9176] | 1.0008 [0.9991, 1.0016] | 0.9976 [0.9891, 1.0079] |
| No live | short-large | 0.9332 [0.3577, 1.0970] | 0.9469 [0.9444, 1.0818] | 0.9559 [0.9358, 0.9893] | 1.0011 [1.0002, 1.0029] | 0.9985 [0.9938, 1.0066] |
| No live | stream | 0.9974 [0.9893, 1.0151] | 1.0000 [0.9500, 1.2667] | 1.0645 [0.9655, 1.1852] | 1.0000 [1.0000, 1.0000] | 0.9922 [0.9855, 0.9989] |
| No live | large | 1.1463 [0.9311, 1.1470] | 1.0000 [1.0000, 1.5000] | 1.0556 [0.9474, 1.0588] | 1.0000 [1.0000, 1.0000] | 1.0021 [0.9989, 1.0021] |
| No live | tool | 0.9856 [0.9631, 1.0766] | 1.0000 [1.0000, 1.1667] | 1.0556 [0.9500, 1.1667] | 1.0000 [1.0000, 1.0000] | 0.9976 [0.9881, 0.9977] |
| No live | cancel | 1.0052 [0.9474, 1.0415] | 1.2500 [1.0000, 1.3333] | 0.9375 [0.8824, 1.1176] | 1.0000 [1.0000, 1.0000] | 0.9942 [0.9936, 0.9987] |
| Live | short-small | 1.0420 [0.9655, 1.4583] | 1.0366 [0.9646, 1.4238] | 0.9833 [0.9600, 1.2471] | 1.0019 [1.0018, 1.0035] | 1.0021 [1.0006, 1.0059] |
| Live | short-large | 0.9668 [0.8977, 0.9791] | 0.9762 [0.8969, 0.9782] | 0.9112 [0.8888, 0.9650] | 0.9984 [0.9982, 1.0018] | 1.0125 [1.0030, 1.0263] |
| Live | stream | 1.0117 [0.9676, 1.0178] | 1.1111 [0.7619, 1.1765] | 1.0000 [0.8182, 1.1000] | 1.0000 [1.0000, 1.0000] | 1.0151 [1.0053, 1.0212] |
| Live | large | 1.0206 [0.9205, 1.1554] | 1.0000 [0.8571, 1.5000] | 0.9412 [0.8947, 1.0556] | 1.0000 [1.0000, 1.0000] | 0.9971 [0.9969, 1.0032] |
| Live | tool | 1.0125 [0.9578, 1.1409] | 1.0000 [0.8750, 1.2222] | 1.0476 [0.9545, 1.0909] | 1.0000 [1.0000, 1.0000] | 0.9905 [0.9807, 1.0114] |
| Live | cancel | 0.9449 [0.8806, 1.0724] | 1.0000 [1.0000, 1.2000] | 1.0000 [0.8947, 1.0000] | 1.0000 [1.0000, 1.0000] | 0.9967 [0.9904, 1.0020] |

The targeted gains are real within these samples: no-live `short-small` improves wall and both CPU measures in all three pairs, with median wall ratio 0.8406 and complete CPU ratio 0.9043. Live `short-large` likewise improves wall and both CPU measures in all three pairs, with wall ratio 0.9668 and complete CPU ratio 0.9112. These observations support retaining the joint change as a targeted improvement, not a broad nonregression claim.

The unfavorable and unresolved outcomes remain part of the retention decision:

- Live `short-small` wall ratios are 1.4583, 1.0420 and 0.9655. The first pair also worsens per-turn CPU by 42.4% and complete CPU by 24.7%; there is no evidence-based cause established for this result, and it is not discarded as a wall-only outlier.
- No-live single-turn `large` has two wall regressions of about 14.6%, with complete CPU ratios 1.0588 and 1.0556 in those pairs. Per-turn CPU is only four to six clock ticks here, limiting fine-grained interpretation without erasing the wall regression.
- Complete CPU medians increase by 6.45% for no-live `stream`, 5.56% for no-live `tool`, and 4.76% for live `tool`; each has two unfavorable pairs. Live sampled anonymous PSS rises in all three `short-small`, `short-large` and `stream` pairs, with median increases of about 0.21%, 1.25% and 1.51% and a largest paired increase of 2.63%.
- Process-attributed write volume is essentially unchanged. No useful write-volume reduction is established for D.
- The favorable no-live `short-large` pair with wall ratio 0.3577 is also retained. Its CPU ratio is approximately 0.9469; it cannot be presented as a representative 64% speedup.

The predeclared retention condition required correctness, useful target gains and no consistent material wider-cost regression. It did not define a numeric materiality cutoff or a required count of unfavorable pairs. Retention is therefore an explicit engineering judgment with these adverse observations, not a mechanically proven gate pass. Only three pairs per case and mode were collected; log-ratio Student-t intervals depend on unverified distribution/independence assumptions. Self CPU uses 100 Hz process counters, and through-cleanup CPU includes startup, harness work and verification. `write_bytes` is process attribution, not device I/O; PSS/RSS are maxima of sampled observations, not true peaks or allocation counts. Recorded ordering and identity checks do not independently establish absence of unrelated machine scheduling activity. [Full retention qualifications](evidence/continuation-20261027/d-retention-audit-notes.txt).

## Rejected E: defer bounded request scalar field indexes

E was separately staged on D and built at `a39f343b54a46c8e5527dad88842cb8a6d46691b`, with `src` tree `7f4952a1e2473ce1a97e2622c742789775a685f7`. For a request scalar of at most 16 KiB—the frozen request with an empty `messages` placeholder, not its complete selected history—it retained the existing authenticated original byte index and omitted the eager per-field Fact/child index. Larger scalars retained the indexed representation. It kept write-time numeric/depth, duplicate navigable path and locator checks, existing page hashes/root/native/frontier bindings, and the shared Fact/preview semantics. It did not add an independent original-capture witness. Inspection reconstructed one resident Source per external operation, while individual field/child lookups repeatedly traversed its metadata. The trial required schema 77 without migration; its rejection restored D and schema 76.

Correctness checks and full HTTP request equivalence did not establish acceptable inspection cost. The ordinary runtime lifecycle benchmark exercises reconstruction and canonical reads but does not cover all explicit request scalar field/page reads, so both the E/D runtime matrix and a separate Reader diagnostic were retained. E's ordinary runtime matrix has the same six cases, two modes and three alternating pairs as D's experiment. The independent audit checked all 72 records and 1,692 reconstructed HTTP request bodies, including 846 fully equal paired requests. **Every table value below is E/D paired-ratio median [minimum, maximum across three pairs]**, with lower values favoring E. This is the incremental E trial on top of D; D's accounting/readiness gains must not be attributed to E. [E audit](evidence/continuation-20261027/e-retention-audit.json).

| Subscriber | Case | Mean turn wall | Turn self CPU/turn | Through-cleanup self CPU | Process writes | Sampled anonymous PSS |
|---|---|---:|---:|---:|---:|---:|
| No live | short-small | 1.0726 [1.0145, 3.1592] | 1.0753 [1.0095, 1.1809] | 1.0882 [0.9947, 1.1637] | 0.9384 [0.9369, 0.9388] | 1.0029 [0.9906, 1.0195] |
| No live | short-large | 0.9009 [0.8797, 0.9699] | 0.9792 [0.8990, 1.0000] | 0.9608 [0.9272, 1.0183] | 0.9413 [0.9407, 0.9429] | 0.9918 [0.9906, 0.9939] |
| No live | stream | 0.9726 [0.9600, 0.9892] | 0.7222 [0.6500, 1.0000] | 0.8065 [0.7879, 0.9032] | 0.8913 [0.8913, 0.8913] | 0.9889 [0.9881, 0.9976] |
| No live | large | 0.9364 [0.7707, 1.2891] | 1.2500 [0.8000, 1.5000] | 0.9444 [0.8889, 1.1875] | 0.8847 [0.8847, 0.8847] | 0.9982 [0.9980, 0.9984] |
| No live | tool | 0.8793 [0.7873, 1.1128] | 0.8333 [0.7500, 1.1429] | 0.9474 [0.8333, 1.0000] | 0.8396 [0.8396, 0.8396] | 0.9783 [0.9774, 1.0005] |
| No live | cancel | 0.8285 [0.8063, 1.0448] | 0.7500 [0.6667, 1.0000] | 1.0000 [0.8824, 1.1333] | 0.8737 [0.8737, 0.8737] | 0.9932 [0.9531, 1.0105] |
| Live | short-small | 0.9248 [0.6712, 1.2907] | 0.9581 [0.6791, 1.2961] | 1.0032 [0.7601, 1.1712] | 0.9356 [0.9355, 0.9369] | 0.9875 [0.9835, 0.9982] |
| Live | short-large | 0.9664 [0.8700, 1.0217] | 0.9943 [0.8856, 1.0386] | 0.9772 [0.9561, 1.0196] | 0.9424 [0.9395, 0.9448] | 0.9917 [0.9889, 1.0013] |
| Live | stream | 0.9773 [0.9676, 1.0036] | 0.8000 [0.7500, 0.9444] | 0.8710 [0.7879, 1.0690] | 0.8913 [0.8913, 0.8913] | 0.9958 [0.9920, 1.0032] |
| Live | large | 0.8524 [0.8268, 1.3066] | 1.0000 [1.0000, 1.1667] | 1.0556 [0.9474, 1.0556] | 0.8847 [0.8847, 0.8847] | 0.9933 [0.9925, 0.9957] |
| Live | tool | 0.7892 [0.7665, 0.8796] | 0.8182 [0.8000, 1.0000] | 0.8800 [0.8400, 1.0435] | 0.8396 [0.8396, 0.8396] | 0.9917 [0.9868, 0.9956] |
| Live | cancel | 0.7594 [0.7578, 1.0232] | 1.0000 [0.6000, 1.0000] | 0.9444 [0.9444, 1.0625] | 0.8737 [0.8737, 0.8737] | 0.9956 [0.9915, 1.0004] |

E reduced process-attributed writes in all 36 runtime pairs: case medians are approximately 5.8%–16.0% lower. Several runtime outcomes also favor it: no-live `short-large` and `stream`, and live `tool`, improve wall in all three pairs; no-live `stream` improves complete CPU in all three pairs. These benefits remain in the record despite rejecting the trial.

The runtime outcomes are not uniformly favorable. No-live `short-small` worsens wall in all three pairs (1.0726, 1.0145, 3.1592) and per-turn CPU in all three (1.0753, 1.0095, 1.1809); its lifecycle wall also worsens in all three. The unusually large wall ratio remains included without a causal attribution. No-live `large` has a per-turn CPU median of 1.25; live `large` has complete CPU median 1.0556 with two unfavorable pairs. The favorable live `short-small` wall median includes a pair at 1.2907, and live `short-large`, `stream`, `large` and `cancel` also each retain an unfavorable wall pair. Total sampled PSS and RSS increase in all three no-live/live `large` pairs and live `cancel` pairs, even where anonymous PSS medians are favorable. Exact adverse pairs, all eight metrics, and the same short-run counter/statistical limitations are retained in the [E audit](evidence/continuation-20261027/e-retention-audit.json).

## Reader diagnostic that prevented retaining E

The Reader diagnostic compares **Original versus forced Indexed within the same E release test binary**, using the same underlying scalar bytes and the actual composed `field`, `page`, root-preview and byte-range implementations. It is not a B/E product benchmark. The small fixture has 1,059 bytes, 78 geometry nodes and 32 tool-schema properties. The near-bound dense fixture has exactly 16,383 bytes, 1,256 nodes and 621 properties. Before timing, both representations must return identical outputs; the 32-field page is checked against the fixture and the 257-byte range against original bytes `[17..274]`.

Each fixture/state/operation has **seven alternating-order pairs, each with 32 reads per representation**, or 224 reads per representation per row. There are 224 batches and 7,168 total reads across the diagnostic. A statement such as “7/7 unfavorable” below refers to all seven **paired 32-read median comparisons**, not seven facts, fields or individual read calls. Warm mode primes both representations. Reopened mode opens a fresh SQLite connection for every read, records opening time separately, and excludes it from Reader latency; it does not clear the OS page cache. The table gives observed per-read median milliseconds and their ratio, plus the range of the seven paired median ratios. [Raw Reader observations](evidence/continuation-20261027/candidate-e-reader.json), [independent calculations](evidence/continuation-20261027/reader-audit.json), [audit notes](evidence/continuation-20261027/reader-audit-notes.txt).

| Fixture | Connection | Operation | Original median ms | Indexed median ms | Ratio of read medians | Paired median-ratio range | Unfavorable pairs |
|---|---|---|---:|---:|---:|---:|---:|
| small | warm | field | 0.073079 | 0.025125 | 2.9085 | [2.8427, 2.9395] | 7/7 |
| small | warm | page32 | 0.547149 | 1.373079 | 0.3985 | [0.3967, 0.4005] | 0/7 |
| small | warm | root_preview | 0.556030 | 1.329878 | 0.4181 | [0.4151, 0.4193] | 0/7 |
| small | warm | range257 | 0.081232 | 0.031552 | 2.5746 | [2.5579, 2.5903] | 7/7 |
| small | reopened | field | 0.152374 | 0.097531 | 1.5623 | [1.5092, 1.6306] | 7/7 |
| small | reopened | page32 | 0.641406 | 1.484566 | 0.4320 | [0.4281, 0.4349] | 0/7 |
| small | reopened | root_preview | 0.664436 | 1.450561 | 0.4581 | [0.4545, 0.4609] | 0/7 |
| small | reopened | range257 | 0.164169 | 0.102730 | 1.5981 | [1.5655, 1.6397] | 7/7 |
| near_16k_dense | warm | field | 0.940450 | 0.040311 | 23.3298 | [23.1251, 23.5079] | 7/7 |
| near_16k_dense | warm | page32 | 7.056711 | 1.726424 | 4.0875 | [4.0396, 4.1438] | 7/7 |
| near_16k_dense | warm | root_preview | 6.695322 | 1.549074 | 4.3221 | [4.3000, 4.3495] | 7/7 |
| near_16k_dense | warm | range257 | 0.972283 | 0.052443 | 18.5396 | [18.4041, 18.6761] | 7/7 |
| near_16k_dense | reopened | field | 1.032874 | 0.108900 | 9.4846 | [9.2528, 9.6813] | 7/7 |
| near_16k_dense | reopened | page32 | 7.162802 | 1.854466 | 3.8625 | [3.8375, 3.8882] | 7/7 |
| near_16k_dense | reopened | root_preview | 6.811473 | 1.688295 | 4.0345 | [4.0091, 4.0491] | 7/7 |
| near_16k_dense | reopened | range257 | 1.068304 | 0.124247 | 8.5982 | [8.4030, 8.7534] | 7/7 |

The small fixture improves page/root-preview latency to about 0.40–0.46 times Indexed, while its single-field/range medians increase by roughly 48–62 microseconds. The dense fixture is unfavorable in every paired group for all eight state/operation combinations. Its median field/range penalty is approximately 0.90–0.94 ms, and its page/root-preview penalty approximately 5.12–5.33 ms. The largest paired ratio is 23.5079 for a warm dense field read. The largest Original observation is an 8.0357 ms warm dense page read; that cell's p95 is 7.1048 ms. Both favorable and unfavorable results are retained.

The implementation explains a plausible source of this density sensitivity: each Original Fact/child lookup walks the complete metadata tree, so page32 performs many full metadata traversals after rebuilding and validating the Source. This is a structural explanation, not a measured per-function attribution. A narrow metadata lookup reusing the existing layout could remove repeated path construction, but preserving canonical aliases, original parent/ordinal evidence and exact preview behavior would require another reviewed and measured candidate; it would still retain per-operation Source reconstruction/validation. That additional change was not implemented in this iteration.

These stable explicit-read regressions, together with the unfavorable runtime cases, prevented retaining E despite its write-volume and other runtime improvements. The trial was reverted; the E build, runtime observations, failed/setup diagnostics, successful validation, and Reader evidence remain available for review. Reader limits remain explicit: this is a release test binary with `cfg(test)` counters active, an empty native selection, fixed early tool-schema fields, a warm OS cache, and returned-payload destruction excluded from measured Reader latency. The 100 Hz `/proc/self/stat` CPU ticks are too coarse for precise per-read CPU claims; zero observed ticks do not mean zero CPU. These results do not establish overall production performance ratios.

## Correctness, failure resolution and evidence

D correctness validation passed **1,312 Rust tests, one ignored**, using release dependencies with root opt-level 0 and all features. Declaration-placement lint cleanup followed; the ordinary O3 D build and product verification are separately identified. All-target/all-feature Clippy with warnings denied passed. The added DeepSeek harness passed 22 tests, and the existing Python harness passed 48. E separately passed 1,319 Rust correctness tests, then 1,102 ordinary O3 tests plus the ignored Reader diagnostic after final lint cleanup; E's correctness does not override its rejected performance tradeoff. [Validation record](evidence/continuation-20261027/validation.json).

The first optimized E test copies omitted `bash-supervisor` and `interactive-supervisor`, causing Python capability preparation and a process-terminal test to fail. Both same-build companions were preserved with separate hashes. Original-artifact and repaired-copy runs traced real uv/FastMCP execution successfully; all five optimized groups then passed. Failures, diagnostics and successful reruns remain archived. A brief overlap between two **functional** verification runs is documented and was outside all performance measurement. These were test-launch layout failures, not silently skipped tests or discarded performance samples. [Diagnosis](evidence/continuation-20261027/python-capability-diagnosis.txt).

No full repository CI, new browser E2E, UI-suite rerun or main merge is claimed. The earlier report records its own UI validation. Current changes and evidence do not establish merge readiness.

The archive contains raw accepted and rejected-trial products, failed functional/setup attempts, original compressed SDK logs, build identities, command plans, independent readers and validation. Executables, dependency checkouts and node_modules remain outside Git. Archive details, hashes and extraction results are recorded in [the evidence README](evidence/continuation-20261027/README.md). The standalone numeric readers do not import producer statistics. After extraction, `--raw-only`/binary-verification-off deliberately distinguishes archived arithmetic checks from original-host actual-binary identity checks.
