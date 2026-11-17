# Joint checkpoint, request and event cost reduction

2026-10-27. Four source-derived reductions were implemented and measured together.
The result supports retaining the combined change for live repeated requests and
smaller durable records, but **does not establish overall Pi Durable parity or
all-scenario nonregression**. Live short-request wall medians improve 18.78% with
small history and 9.66% with large history against the previous G/H source.
Complete process writes improve about 5% for repeated requests. The initial
no-live large-history cell is slower in all three pairs; its separate confirmation
is reported below. Cold timing, CPU and memory results are mixed.

Measured source: `123accb38fd8cf1e8f88583c1858d514cb453870`; source tree
`65d050f7f3c72007309e2e61b0324dc52227ba40`. Ordinary O3 binary SHA256:
`6899c8fad1fc85e8ffd36a33b92e85cec877cda3689bb26a03c6f07c658a3d43`. Direct baseline: G/H
`063ef7a9b713e76efad6dc4e6b7122454023c67f`, binary
`cce30715eed4b1700c5b3c5fee2a93db1fb4ca49c8847cf36dfe995e4470e9b0`.
These are direct measurements, not products of gains from earlier rounds.

## Changes grounded in the reference source

- **Checkpoint capture:** reuse immutable normalized captures for 19 domains and
  their sufficient admission/fit charges. Unchanged domains avoid repeated JSON
  normalization and geometry checks; sufficient aggregate proofs skip the full
  checkpoint fit walk. Current content/cut/frontier metadata is resolved afresh.
  Residual-budget boundaries preserve the original capture and eviction order.
  Generic reuse excludes floats and nonempty maps; native capability/trace owners
  use identity. [Implementation](../../src/runtime_client/projection/prepared_capture.rs)
- **Native request configuration:** persist the ordered system sections, effective
  system prompt and tool definitions once per exact content digest. Per-request
  metadata and selected-history bindings remain exact. Borrowed serialization
  removes the initial full snapshot clone. Reads still fetch, hash and fully decode
  the shared payload; this is primarily write reduction, not elimination of native
  configuration CPU. Complete reconstructed content remains charged to the original
  64 MiB bound. [Implementation](../../src/durable/sqlite/native_request.rs)
- **Derived event records:** encode statistics and execution in closed positional
  JSON tuples. Identical conversation/local statistics are stored and folded once;
  distinct lineage totals and local duration remain separate. Execution preserves
  its former admission check. Statistics keeps the 16 KiB private storage cap over
  the smaller encoding, so some optional data formerly too large now fits; public
  response bounds remain unchanged. [Statistics](../../src/durable/sqlite/observation_statistics.rs),
  [execution](../../src/durable/sqlite/observation_execution/storage.rs).
- **Fixed SQL:** reuse prepared plans for response, aggregate and occupancy paths.
  Occupancy prepares once and rebinds three probes. Query results, transaction
  evidence, corruption checks and historical-cut ownership are not cached across
  calls. The existing statement-cache capacity remains 64.

Pi is pinned at `90d863f7c1183b965f099855715d6e88c7b8a52c`. Its
[prompt planner](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/prompt.ts#L66)
emits changed system/tool entries and request checkpoints retain invocation fields
and a history cutoff. DeepSeek is pinned at
`687f282e8625c6b8d11ff58211a90524be2e4317`; its
[agent loop](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/core/agent-loop/src/agent.ts#L626)
omits unchanged ordinary headers. Both retain comparison/serialization work and
incrementally derive history. The archive's `source-comparison.md` gives exact
source locations, changes and residual work for all four mechanisms.

SQLite remains WAL/NORMAL, matching the pinned Pi backend's pragmas. This change
does not consolidate native commit boundaries. Schema **78** has no migration or
compatibility decoder; older stores are rejected under repository policy.

## Correctness and experiment scope

The final recorded suite passed **1,355 tests, one ignored**, plus formatting and
all-target/all-feature Clippy with warnings denied. Six native configuration,
six capture and four compact-layout regressions cover exact bytes, reordered or
changed values, residual budgets, historical cuts, reopen/retry, coherent malformed
payloads, trigger rollback and corruption. All failed compile/test/Clippy rounds
are retained. Final source hashes are identical before/after checks and match the
committed source. Correctness uses release dependencies with the root crate at
opt-level 0 and all features; performance uses a fresh ordinary O3 build.

The original formal matrices contain **180 products**: 72 direct, 72 fresh Pi,
and 36 three-engine runs. The direct and Pi matrices each validate 846 requests.
Same-engine comparisons use the complete request oracle; cross-engine comparisons
use the common core because native prompts/tool inventories differ (Rust seven,
Pi four). Three-engine runs add 12 native DeepSeek close/resume checks. Diagnostics
run separately. CPU counters have 10 ms granularity; PSS is sampled, not a continuous
peak; process `write_bytes` is not device traffic. Full repository CI/browser E2E
and a main merge were not performed; the PR remains draft with existing conflicts.

## Direct combined result against G/H

Medians of three **paired joint/GH ratios**, not ratios of medians. Below one favors
the joint version. Full CPU/writes include setup, verification and cleanup. Complete
ranges and every raw sample remain in `joint-audit.json`.

| Mode | Case | Turn wall | Full CPU | Process writes | Sampled anon PSS |
|---|---|---:|---:|---:|---:|
| No live | Short, small history | 0.9882 | 1.0056 | 0.9424 | 0.9998 |
| No live | Short, large history | 1.0215 | 1.0156 | 0.9513 | 0.9977 |
| No live | Stream | 0.9954 | 0.9310 | 0.9964 | 1.0154 |
| No live | Large | 0.8816 | 1.0000 | 1.0020 | 1.0051 |
| No live | Tool | 0.9269 | 0.9500 | 0.9900 | 0.9893 |
| No live | Cancel | 0.9202 | 1.0000 | 1.0010 | 0.9992 |
| Live | Short, small history | 0.8122 | 0.9178 | 0.9440 | 0.9975 |
| Live | Short, large history | 0.9034 | 0.9297 | 0.9488 | 0.9906 |
| Live | Stream | 1.0213 | 1.1481 | 0.9946 | 1.0049 |
| Live | Large | 0.9166 | 0.9444 | 1.0020 | 1.0051 |
| Live | Tool | 1.0383 | 1.0000 | 0.9900 | 1.0080 |
| Live | Cancel | 0.9170 | 1.1250 | 1.0010 | 1.0022 |

All six live repeated-request pairs improve wall and full CPU; all twelve repeated-
request pairs reduce writes. This does not generalize to every cell. No-live large-
history wall ratios are 1.012–1.200, median 1.022, with all three slower. No-live
small-history full CPU median is 1.006. Live stream full CPU median is 1.148 and
live cancel 1.125; live tool wall is 1.038. Cold large/cancel writes increase
approximately 0.2%/0.1% in both modes. Several memory cells are higher in all three
pairs, including no-live stream anonymous PSS (median +1.54%). These remain in the
accepted dataset; no universal nonregression is claimed.

The no-live large-history concern triggered **seven separate confirmation pairs**
(14 additional products and 672 full request comparisons), run after the
completed diagnostics with no overlap. They are not pooled into the original
matrix. Wall: 6 lower, 0 equal, 1 higher;
median ratio 0.927 [0.849–1.159]. Full CPU: 0.965 [0.925–1.036]; writes: 0.949 [0.946–0.953].
All seven ratios and memory results are retained. This selected cell is post-hoc
confirmation, not a full seven-pair parity gate. The overall formal total is 194.

## Logical storage versus process writes

Separate synchronized database snapshots bracket 32 measured turns after the
8/64-turn warmup. These are sums of logical column values, not WAL or device bytes.
The shared configuration row already exists before measurement and adds **zero
rows/bytes** during either steady interval.

| Added logical value bytes | Small: GH → joint | Large: GH → joint |
|---|---:|---:|
| Native snapshot JSON | 383,770 → 38,426 (-89.99%) | 383,840 → 38,496 (-89.97%) |
| Event statistics | 231,628 → 73,229 (-68.39%) | 233,013 → 74,003 (-68.24%) |
| Event execution | 139,373 → 46,909 (-66.34%) | 139,543 → 47,063 (-66.27%) |
| Indexed request pages | 971,739 → 971,839 (+0.01%) | 972,638 → 972,748 (+0.01%) |
| All table column values | 3,161,197 → 2,564,940 (-18.86%) | 3,269,458 → 2,672,096 (-18.27%) |

Native snapshot JSON falls about 90%, statistics 68% and execution 66%; all table
logical values fall about 18–19%. Each interval still adds 256 native events and
about 0.80 MB of event-column values. Indexed request pages, source tree nodes,
other event layouts, indexes and commit boundaries remain. SQLite can coalesce
same-transaction dirty pages; fewer UPDATEs, logical bytes, WAL frames and process
writes are different quantities. There is **no fresh WAL/transaction count** in
this round. The previous F count must not be relabeled as a joint measurement.

## Separate CPU diagnostics and residual costs

One baseline and one joint 250 Hz live/large-history profile, each with a separate
control run, passed independent raw-count, retained symbol-report consistency,
identity and timing-isolation checks. Inclusive stacks overlap across rows; sample counts are not call
counts and these two profiles do not isolate each change's causal speedup.

| Inclusive sampled stack | GH / 360 samples | Joint / 328 samples |
|---|---:|---:|
| PublicationDraft::encode | 124 (34.44%) | 75 (22.87%) |
| Checkpoint capture, within publication | 82 | 40 |
| Full checkpoint fit, within publication | 14 | 0 |
| Final structural length, within publication | 26 | 32 |
| SQLite prepare | 61 (16.94%) | 56 (17.07%) |
| ReadDomainCut::read_with_trace | 34 | 39 |
| Request read-layout indexing | 19 | 22 |

Capture/fit/length are disjoint **within publication**, with another 2/3 publication
samples respectively. Zero sampled fit work is not proof that the path never runs.
The new profile supports reduced publication capture/fit work; it does not show
elimination of final schema-aware encoding or a clear SQLite-prepare improvement.
The whole publication share must not be called the cost of final serialization.

Prepared capture still compares domain values, checks serialization shape and
clones bounded data; new cache misses also prepare and retain a copy. Ordinary
deltas still construct a complete checkpoint then retain only its metadata. Those
added costs may offset reuse in no-live workloads; the current live-only samples
do not establish the cause of the no-live regression. Exact structural lengths
remain necessary for replay retention, and unconstrained native u64 values use
string encoding even when numerically small, so plain serde is not interchangeable.

The append path still constructs, writes and then rereads multiple derived facts.
Static schema inspection finds 32 observation columns and 40 explicitly declared
event-related indexes (28 direct plus 12 presentation; autoindexes excluded).
These are schema counts, **not** indexes touched per event or WAL frame counts.
A shared append-scoped fact assembly could reduce this work, but blindly merging
updates would change independent optional-layout failure survival and post-write
corruption checks. `remaining-append-work.md` and `remaining-publication-work.md`
record the exact next architectural targets and their constraints. No further
helper-level speedup is invented from these observations.

## Fresh comparison with Pi

Fresh paired joint/Pi ratios; each cell retains three pairs and its full range.
Warm wall covers 32 measured turns; full CPU/writes include warmup and cleanup.

| Mode | Case | Wall median [range] | Full CPU median | Process writes median |
|---|---|---:|---:|---:|
| No live | Short, small history | 8.078 [7.670–8.084] | 1.9278 | 7.2394 |
| No live | Short, large history | 6.579 [5.551–8.124] | 3.4820 | 8.1521 |
| No live | Stream | 0.916 [0.907–0.920] | 0.2569 | 5.0182 |
| No live | Large | 0.476 [0.455–0.522] | 0.2833 | 5.8663 |
| No live | Tool | 0.586 [0.561–0.655] | 0.3175 | 5.3792 |
| No live | Cancel | 0.445 [0.419–0.476] | 0.2712 | 6.0566 |
| Live | Short, small history | 8.116 [6.510–9.196] | 2.4227 | 7.1791 |
| Live | Short, large history | 9.303 [2.157–9.443] | 4.3497 | 8.1299 |
| Live | Stream | 0.921 [0.909–0.989] | 0.2437 | 5.0182 |
| Live | Large | 0.559 [0.517–0.790] | 0.3000 | 5.8663 |
| Live | Tool | 0.847 [0.668–1.164] | 0.3710 | 5.3792 |
| Live | Cancel | 0.415 [0.355–0.519] | 0.2903 | 6.0566 |

Warm wall medians remain **6.58–9.30× Pi**, full CPU **1.93–4.35×**, and process
writes **7.18–8.15×**. Every warm wall/full-CPU/write sample exceeds one. The live large-history range
includes a 2.157× pair; it is retained, not used to replace the 9.303× median.
Cold wall medians are lower than Pi, but live tool has an adverse 1.164× sample,
and all cold write ratios remain above five. Lifecycle, memory and every metric
range are in the raw audit. These results do not meet the all-metric, both-mode,
at-least-seven-pair parity gate.

## Native DeepSeek reference

The three-engine cohort rotates order across three repetitions. Values below are
medians for the **measured turn window**, not full lifecycle values or ratios.

| Case | Engine | Wall ms/turn | CPU ms/turn | Process MiB written/turn |
|---|---|---:|---:|---:|
| Short, small history | rust | 33.734 | 45.938 | 1.592407 |
| Short, small history | pi | 3.900 | 7.812 | 0.214111 |
| Short, small history | deepseek | 15.274 | 17.500 | 0.020508 |
| Short, large history | rust | 31.059 | 42.188 | 1.929077 |
| Short, large history | pi | 4.102 | 7.188 | 0.213989 |
| Short, large history | deepseek | 15.662 | 17.812 | 0.020386 |
| Large | rust | 50.976 | 60.000 | 1.519531 |
| Large | pi | 93.930 | 130.000 | 0.281250 |
| Large | deepseek | 165.875 | 200.000 | 0.031250 |
| Stream | rust | 1589.079 | 170.000 | 1.914062 |
| Stream | pi | 1721.891 | 630.000 | 0.492188 |
| Stream | deepseek | 1607.122 | 320.000 | 0.027344 |

DeepSeek uses pinned native SDK source through TSX, 25 native tools and native
session logs, with the recorded generated-Typer-loader profile adjustment. It
lacks partial-token-text publication. Its SDK idle signal does not await the final
JSONL flush/fsync, while the shipped headless entry point explicitly awaits
[session flush after idle](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/bundle/headless/src/index.ts#L379).
Checkpoint barriers before dispatch/tools/next-step remain in its source.
All 12 close/resume checks pass, but they do not establish power-loss durability.
These absolute SDK costs are an additional reference, not an equal-durability
ranking or built-CLI startup comparison.

## Recomputable evidence

[The evidence bundle](evidence/joint-major-runtime-20261027/README.md) contains
all successful and failed checks, complete baseline/candidate build logs and
identities, all formal and diagnostic raw products, native DeepSeek compressed
sessions, source analysis and independent auditors. The manifest records every
member hash and size; compiled products are excluded. Both auditors support
extracted raw-only recomputation and explicitly do not claim actual executable
or current-source verification in that mode.

The archive contains **1,450 payload files**, 1,788,820,032 uncompressed bytes,
in a 9,300,904-byte archive. Archive SHA256:
`6569c3c9afade988d29b076ee7779224fa47472cfd920d482cb83990fd73b6ac`.
[Portable verification](evidence/joint-major-runtime-20261027/portable-verification.json)
passed after extraction into a fresh directory: every member hash/size, both
raw-only auditors, all 194 formal products and diagnostic numerical results agree
with the retained strict reports. The raw inventory and hashes remain unchanged
after auditing. This verifies retained evidence integrity and recomputation,
not a fresh product execution or reproducible build.
