# Shared indexed request configuration and bounded response reads

2026-10-27. G/H removes repeated request-index construction and repeated
response-owner reads while preserving eager historical inspection. The completed
F/GH comparison supports retaining this targeted reduction. It does **not** show
universal nonregression. Fresh comparisons still show a large warm-request gap to
Pi Durable; native DeepSeek is an additional reference with a different completion
boundary. Separate CPU samples show that request-index and read-cut work remain
smaller parts of the sampled stacks, while publication capture/encoding remains a
large cost. The samples do not isolate causal speedups.

Measured Rust source: `063ef7a9b713e76efad6dc4e6b7122454023c67f`; production tree:
`ce0d7116a42d6d5eb15f2c7af923e204307a1201`; ordinary O3 binary SHA256:
`cce30715eed4b1700c5b3c5fee2a93db1fb4ca49c8847cf36dfe995e4470e9b0`.
The preserved F baseline is `2800be25a650305d5daf9f543ffbf7d8f62ea79b`.
Schema 77 introduces the stable-fragment registry without a compatibility path or
migration. The PR remains draft and still has conflicts against main.

Evidence is retained in
[shared-request-response-20261027](evidence/shared-request-response-20261027/README.md).
Member paths below refer to its `raw.tar.xz`, including `source-comparison.md` for
the complete source references. The archive includes failed check rounds and
adverse performance samples as well as successful results.

The 8,726,964-byte archive contains 1,445 payload files. Independent extraction
verified every member's size/hash and reproduced all 194 formal products' audit
results and the separate diagnostics in raw-only mode. The
[portable verification](evidence/shared-request-response-20261027/portable-verification.json)
records that result; portable mode does not revalidate unavailable host binaries
or current source trees.

## What the reference implementations avoid

Pi Durable is fixed at `90d863f7c1183b965f099855715d6e88c7b8a52c`.
Its [prompt planner](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/prompt.ts#L66) compares rendered system
sections and ordered tools and emits only required updates. A changed head can
require a new baseline, and reordered tools can require remove/add operations.
Ordinary requests checkpoint model, thinking level, options and an immutable
history cutoff (`harness/generation.ts:64–72,161–194`); they do not persist a fresh
complete tool declaration each time. This still requires comparison work:
`packages/ai/src/utils/transcript.ts:58–65` compares tool declarations through
`JSON.stringify`.

DeepSeek Harness is fixed at `687f282e8625c6b8d11ff58211a90524be2e4317`.
Its [agent loop](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/core/agent-loop/src/agent.ts#L626) omits unchanged ordinary request
headers and appends request context when compared values change. Initial/resumed
requests and new series deliberately retain headers. Ordered schema comparison
still serializes values (`core/session/src/request-header.ts:21–51`), and request
freezing still creates a boundary array. Neither implementation has zero
configuration CPU.

For history, Pi extends a cached context range from its previous tail and still
copies returned arrays ([context.ts:68–108](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/context.ts#L68)). DeepSeek folds
new log entries and derives newly appended nodes until content generation changes,
then returns a copied array ([session/index.ts:984–1004](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/core/session/src/index.ts#L984)). These
patterns avoid repeat work; they do not establish equivalence with Rust's
authenticated arbitrary historical field/page/range reads.

## G and H: removed work and retained obligations

G separates six changing request fields from a stable scalar suffix. Exact encoded
content identifies an immutable, already indexed suffix; changed configuration or
tool order selects a different version. Existing roots retain their own bindings.
This uses the existing fragment format and selected-message composition rather
than a second cache or deferred reader reconstruction.

On an authenticated hit, `composed/scalar.rs:98–167` returns before building a
`Source` index, walking field geometry, or calling `fragments::persist_part` for
the stable suffix. It avoids stable-field `capture_facts`, preview/header encoding,
fact-spool construction, byte/field-page encoding, page hashes and page INSERT
attempts. Previous page deduplication happened after this construction and hashing.
Misses consume the encoded carrier once and retain eager indexing.

G still encodes and hashes the full stable inspection prototype on every request.
It reads/authenticates the registry descriptor and constructs the changing prefix.
Large encodings can spill under the existing bounded-residency policy. Complete
scalar JSON at most 16 KiB still receives original node-count validation. Native
snapshot cloning, JSON encoding, integrity hashes and per-request persistence
remain, as do selected-input augmentation, root checksums, request summaries and
trace/context/catalog indexing. Historical reads still validate indexed pages;
the hit path is not constant-time and is not zero configuration CPU.

H batches numeric response decoration for one bounded page into one store call
and SQLite read transaction. `observation_response.rs:16–145,262–376` validates
the cut and shared member-hole witness once and reuses decoded execution, member,
process, inherited-response and identity results within that call. This removes
repeated store entry and repeated lookup/decoding of the same owners. It does not
remove writes or introduce a cache shared between cuts.

Per-selection ownership, role/tool-proposal checks, inherited membership, execution
holes, response locators, retry identity and statistics still cost work. Distinct
owners require distinct facts, and separate page reads repeat validation. Both
changes preserve existing inspection and corruption semantics; H is not one SQL
statement for an arbitrary page.

Pi and Rust both use SQLite WAL with `synchronous=NORMAL`; G/H does not weaken that
setting. Matching pragmas do not equalize transaction counts or derived storage.
DeepSeek's checkpoint policy awaits flush at dispatch/tool/pre-step boundaries,
and its JSONL backend syncs appended batches. An already materialized flush can
be a no-op. Its SDK idle signal is not equivalent to Rust's durable settlement;
the extra reference must retain that boundary explicitly.

## Completed validation and direct measurements

The recorded successful correctness suite has **1,339 passed, 1 ignored**.
Formatting and all-target/all-feature Clippy checks also passed. All earlier failed check
rounds remain archived. G's differential coverage includes exact bytes, aliases,
field/page geometry, nonempty selected messages, changed configuration, reopening,
corruption and counters proving hit paths skip stable index construction. H covers
bounded ownership reuse and historical-cut/error behavior. The correctness build
is separate from the ordinary O3 performance build.

`gh-direct-audit.json` accepted the strict identity/oracle audit: 72 product runs,
36 F/GH pairs and 846 request comparisons. There are three pairs per case and
subscriber mode. The table gives **medians of paired GH/F ratios**, not ratios of
medians; values below one favor GH. Full CPU includes cleanup and verification.

| Mode | Case | Turn wall | Full CPU | Process writes | Sampled anon PSS |
|---|---|---:|---:|---:|---:|
| No live | Short, small history | 0.9363 | 0.9634 | 0.9375 | 1.0093 |
| No live | Short, large history | 0.8316 | 0.9532 | 0.9474 | 1.0028 |
| No live | Stream | 1.0046 | 0.9677 | 1.0036 | 1.0002 |
| No live | Large | 1.0406 | 1.0000 | 1.0040 | 0.9965 |
| No live | Tool | 0.8021 | 0.8500 | 0.9229 | 0.9947 |
| No live | Cancel | 1.0094 | 1.0588 | 1.0042 | 1.0000 |
| Live | Short, small history | 0.9290 | 0.9323 | 0.9379 | 1.0060 |
| Live | Short, large history | 0.8099 | 0.9141 | 0.9418 | 0.9937 |
| Live | Stream | 1.0142 | 1.0345 | 1.0036 | 0.9989 |
| Live | Large | 1.0331 | 0.9474 | 1.0040 | 1.0013 |
| Live | Tool | 0.9100 | 0.9600 | 0.9229 | 0.9882 |
| Live | Cancel | 1.1000 | 1.0000 | 1.0042 | 0.9924 |

These results are mixed. In particular, the initial live-small pair 1 took
166.9648 ms/turn against F's 45.5441 ms/turn: **3.666× slower**. There is no recorded
off-CPU evidence establishing its cause, so it remains in the dataset. The seven
additional live-small confirmation pairs are separate evidence, not pooled into
the initial cohort: five wall reductions, two increases; median wall −13.77% and
full CPU −8.22%. Sampled anonymous PSS increased in **all seven**, median +1.021%.
`gh-confirmation-audit.json` accepted the separate strict confirmation audit. This does not
justify claiming unchanged memory or all-metric nonregression.

## Why fewer request pages do not mean equally fewer physical writes

Separate synchronized column diagnostics compare each database before and after
32 measured turns. Added logical values in `request_source_pages` fell from
2,691,945 to 971,718 bytes for small history (−63.90%), and from 2,557,784 to
972,637 bytes for large history (−61.97%). These totals include hash and payload
columns. The registry added no new row during either steady interval; it was
already populated. Native `snapshot_json` additions remained 383,770 and 383,840
bytes respectively in both versions. This directly shows which duplicate
representation was removed and which remained.

Each 32-turn interval still adds 256 event rows. Native `event_json` adds about
101 KB, and the complete event columns add about 1.05 MB after including derived
observation statistics, execution/member facts, witnesses and trace fields.
Native event JSON differs by only 10/15 bytes between the independent runs; it is
not byte-identical. Request snapshots still add about 429 KB across all columns,
and request tree nodes add another 358/454 KB. These remaining representations
explain why removing repeated stable pages does not remove the complete storage
cost. The [previous F transaction diagnostic](2026-10-27-write-compute-reduction.md#wal-verification-after-f)
measured 16 commits per turn versus Pi's five; G/H does not consolidate the write
transactions. That count is historical F evidence, not a fresh G/H WAL count.

Logical column bytes are neither WAL traffic nor device writes. Direct process
write ratios improve about 5–8% in the repeated-request/tool cases; cold stream,
large and cancel cases instead have small write increases from setup. SQLite page
allocation, other indexed state, events, snapshots and checkpoints remain. These
diagnostics support the mechanism, not a 62–64% whole-process write-reduction claim.

## Fresh Pi comparison: the remaining gap

`gh-audit.json` accepts all **194 products**: direct 72, fresh Pi 72, three-engine
36 and confirmation 14. The fresh Pi cohort independently checked 846 requests;
these ratios are measured directly, not products of earlier ratios. Each cell
below contains three paired GH/Pi samples. Brackets retain their entire range.
Warm wall covers 32 turns after 8/64 history turns; full CPU and process writes
include warmup and cleanup.

| Mode/history | Turn wall ratio | Full CPU ratio | Process-write ratio |
|---|---:|---:|---:|
| No live/small | 8.365 [7.069–8.874] | 1.917 [1.810–2.053] | 7.675 [7.627–7.676] |
| No live/large | 6.670 [6.355–8.247] | 3.584 [3.489–3.716] | 8.563 [8.531–8.568] |
| Live/small | 10.888 [9.524–20.973] | 2.802 [2.684–3.011] | 7.651 [7.646–7.686] |
| Live/large | 10.538 [9.676–11.016] | 4.724 [4.586–5.236] | 8.568 [8.548–8.568] |

Every warm wall/phase-CPU/full-CPU/write sample exceeds Pi. Fresh live-small pair 2
is **20.973× GH/Pi**, 81.932 ms/turn versus 3.907 ms/turn; seven GH turns exceed
100 ms. This is a fresh cross-engine tail, **not a 20.973× F/GH regression**.

Cold cases have lower wall medians but retain adverse individual samples:

| Mode | Case | Wall ratio, median [range] | Full CPU median | Writes median | Lifecycle median |
|---|---|---:|---:|---:|---:|
| No live | Stream | 0.895 [0.893–0.923] | 0.252 | 5.036 | 0.951 |
| No live | Large | 0.532 [0.474–0.605] | 0.276 | 5.855 | 1.253 |
| No live | Tool | 0.603 [0.527–0.605] | 0.290 | 5.433 | 1.219 |
| No live | Cancel | 0.395 [0.332–0.421] | 0.246 | 6.050 | 1.172 |
| Live | Stream | 0.911 [0.898–0.928] | 0.278 | 5.036 | 0.973 |
| Live | Large | 0.546 [0.523–1.508] | 0.295 | 5.855 | 1.305 |
| Live | Tool | 0.730 [0.572–1.081] | 0.355 | 5.433 | 1.346 |
| Live | Cancel | 0.528 [0.509–1.033] | 0.311 | 6.050 | 1.263 |

Live cold pair 1 is slower for large/tool/cancel; the other two pairs in each
cell are faster. Full CPU is lower in every cold sample, while process writes
remain 4.80–6.05× Pi. Better turn medians do not imply better lifecycle latency,
writes or every observation. Sampled PSS and all metric ranges remain in
`gh-final-cost-analysis.txt`. Three-pair cells plus one selected confirmation cell
do not meet the all-metric, both-mode, at-least-seven-pair parity gate.

## Native DeepSeek reference

The 36-product three-engine cohort rotates execution order three times per case.
It retains 12 Rust/Pi request comparisons and **12 native DeepSeek close/resume
checks**, all accepted. The following native DeepSeek absolute values are medians
[full range] for the measured turn window, not the preceding full-lifecycle scope.
Cross-engine Rust/Pi checks use the common core request oracle; their native
system prompts and tools are not identical (Rust seven tools, Pi four). The
same-engine F/GH comparison uses the full request oracle.

| Case | Idle wall ms/turn | CPU ms/turn | Process MiB written/turn |
|---|---:|---:|---:|
| Short/small history | 15.118 [14.532–15.157] | 17.188 [16.875–17.500] | 0.020508 [0.020508–0.020508] |
| Short/large history | 14.965 [14.649–15.615] | 16.875 [16.875–18.125] | 0.020386 [0.020264–0.020386] |
| Large | 168.728 [162.739–169.256] | 190 [180–200] | 0.031250 [0.031250–0.031250] |
| Stream | 1573.701 [1572.473–1595.867] | 280 [280–310] | 0.027344 [0.027344–0.027344] |

This uses the shipped native SDK source through TSX and 25 tools, with the
unavailable generated Typer loader disabled in the recorded profile patch. It is
not a comparison of built CLI startup. The SDK has no partial-token-text
publication surface. DeepSeek idle does not await final JSONL flush/fsync, so its
small sampled write counts cannot establish equal-durability superiority. Resume
checks verify readable history and continuation, not power-loss durability;
`dispose` uses `allSettled`, and exit zero is not a positive fsync certificate.
CPU counts the extant process tree plus waited children; I/O excludes exited
descendants. Complete three-engine absolute ranges remain in the independent
summary; no unavailable field is filled with zero.

## CPU diagnostics

The independent `diagnostic-audit.json` accepts four CPU/control products and four
column-diagnostic products, separate from the 194 formal products. Each CPU
version has one control and one 250 Hz profile, covering 32 live short turns after
64 seed turns. Actual product/preload hashes, raw profile headers, collapsed
totals and symbol reports agree. The CPU/control products ran after the formal cohorts;
their time windows do not overlap. The correctness build used all features with
the root crate at opt-level 0 and release dependencies; timing and diagnostic
products use the separately built ordinary O3 executable.

| Inclusive stack | F samples / 522 | G/H samples / 496 |
|---|---:|---:|
| `ReadDomainCut::read_with_trace` | 146 (27.97%) | 62 (12.50%) |
| `index_request_read_layouts` | 67 (12.84%) | 34 (6.85%) |
| `sqlite3LockAndPrepare` | 116 (22.22%) | 95 (19.15%) |
| `PublicationDraft::encode` | 109 (20.88%) | 160 (32.26%) |

Inclusive stacks overlap and cannot be added. Profiles include all product
threads and between-turn driver/control work; they exclude off-CPU waits.
Profiling perturbs execution, and one profile per version does not establish
statistical significance. In particular, the larger publication sample count
does not prove that publication became slower. G and H were measured together;
neither this cohort nor source inspection isolates their individual speedups.
These CPU samples cannot establish the cause of the recorded wall-time tails.

The next substantial target is the publication path's repeated whole-checkpoint
work. `PublicationDraft::encode` captures a bounded checkpoint for each frozen
transition, collects content/collection evidence, fits its budget, stamps cuts,
then traverses the structural encoding to check transition size. An ordinary
delta therefore still pays checkpoint construction and fit costs; a replacement
also needs its bounded encoding probe. `checkpoint_unfitted` visits messages,
transcript, trace, attempt, context, resources and other snapshot domains even
though many do not change on that transition. Pi's cached context ranges and
DeepSeek's incremental log folds/observed-sequence projections point toward
reusing unchanged domain captures. Rust already caches selected history, so
adding another generic history cache would miss the observed work.

That architectural reduction must retain historical read cuts, bounded output,
corruption checks and late-reader recovery. SQLite statement preparation and
per-request native snapshots, derived event facts and ownership validation are
also still material. G/H removes proven repeated work without deleting those
features; the measured end-to-end gains remain mixed, and overall Pi Durable
parity has not been achieved.
