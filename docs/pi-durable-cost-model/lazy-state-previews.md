# Lazy native-state previews

Native state indexing retains original segmented bytes and the coordinates needed
for direct field/range/page reads. It now skips recursive preview construction
for every field and stores compact geometry instead of duplicated preview JSON.
The existing root row holds the source kind and extent; no extra descriptor table
or write is added. Navigation remains eager, so cold small reads do not parse or
reindex the full Source. This is the first bounded lazy-observation increment,
not a claim that all observation indexing is deferred.

A requested preview uses the same 32-node budget and 16 KiB original-span rule as
the previous eager implementation. Child lookup uses source/parent/ordinal keys;
individual metadata and byte reads are bounded. Long keys, reserved key paths,
original message overrides, large u64 and pagination retain their existing
meaning. Range-only reads bypass preview generation. Reads neither persist a cache
nor schedule background work; existing errors/readiness remain truthful at the
requested historical cut.

The SQLite development gate advances to 73, and exact table/index checks reject
unsupported layouts without migration, dual writes or fallback. Tests compare all
fixture fields against the independent eager oracle, exercise cold large Sources,
count indexed VM work and returned segment slices, and reject malformed bytes,
geometry and previous layouts. Returned slice counts are not physical-device I/O
measurements. Existing request-capture authentication is unchanged in this stage;
state previews are now derived from committed retained bytes rather than separate
stored preview copies. No stronger independent state-preview witness is claimed.

Validation passed: all-target/all-feature Clippy with warnings denied, all 404
durable SQLite tests, and all 26 Source tests. The four new differential, cold-read,
malformation and format-gate tests also passed separately. Checks and the initial
compile/oracle/lint failures are retained in `evidence/lazy-state-previews/`.

## Isolated release comparison

Compared scoped indexes `d054be08981dcc98c53c5ba83641b387f26442d4`
(binary `475c6ef0081a2ddf6646dd47e29f5d02d7b3ac2d3435506d47de00fffbc11eb6`)
with lazy previews `b5840bd903d2508e693e1a4f80c3dc6173c9e29b`
(binary `e6e3c0109de5202820201b1aa657f2e018223240934131829cb7e5761936a474`).
Fresh root artifacts, matching Rust toolchain/build settings and five fixture
hashes were verified. All 36 samples / 846 full ordered requests / 18 paired
comparisons passed, with three interleaved fresh-process pairs per case. No
compiler or profiler overlapped these measurements.

Median within-pair after/before ratios:

| Case | Turn wall | Turn CPU | Lifecycle writes | Sampled anonymous PSS |
| --- | ---: | ---: | ---: | ---: |

| short-small | 0.9842 | 0.9932 | 0.9876 | 1.0173 |
| short-large | 0.9963 | 0.9909 | 0.9894 | 1.0135 |
| stream | 0.9964 | 1.0435 | 0.9941 | 0.9909 |
| large | 1.0022 | 1.0455 | 0.9960 | 1.0081 |
| tool | 0.9451 | 0.8571 | 0.9894 | 0.9921 |
| cancel | 0.9789 | 0.8571 | 0.9950 | 1.0090 |

Lifecycle writes improve modestly across all cases, approximately 0.4–1.2%.
There is no general CPU or memory gain: stream/large turn CPU ratios are
1.0435/1.0455, stream lifecycle CPU is 1.0270, and short-small anonymous PSS is
1.0173. Every adverse sample is retained. CPU includes runtime and driver at
10 ms self-CPU resolution; small CPU ratios are coarse. Memory is sampled
RSS/PSS, not peak or allocation count. Writes are process-attributed, not
physical-device writes. Lifecycle includes initialization, verification,
checkpoint/close/reopen and cleanup. Evidence output follows counter capture;
provider startup/control remains explicit, with sibling provider lifetime
through product cleanup. No unrelated medians are subtracted.

First reads trade stored previews for indexed metadata and byte access. The
cold 2 MB fixture uses an 8-page SQLite cache: the u64 field preview requires
one 20-byte segment slice and fewer than 500 VM steps; a two-element tail page
requires fewer than 1000 steps. A distant 32-byte range returns one segment
slice and constructs no preview. These are tested upper bounds and returned
slice counts, not physical-device I/O or measured first-read latency. The
differential fixture verifies capture constructs zero previews and skips each
retained field. Navigation geometry is still built eagerly; this does not
eliminate all Source indexing work.

The separate diagnostic passed both full ordered oracles. It records 16,548 to
16,379 WAL frames (68,177,760 to 67,481,480 frame bytes); endpoint-dbstat frames
attributed to state_observation_fields fall from 1167 to 989. Both sides add 3008
state fields and 256 events. Write-commit markers differ, 672 versus 671: this
instrumented comparison does not isolate the schema change from scheduling or
partial-publication work. No pure-preview saving is inferred from the total.
The diagnostic retains tracing/snapshot overhead, complete straces, endpoint
owners and unmapped/mixed pages; endpoint ownership cannot exclude intermediate
page reuse. It is outside headline timing and supplies no exclusive CPU share.

`evidence/lazy-preview-measurements/` contains compact complete records, scripts,
build identity/logs, summaries and a SHA-256 manifest. Original evidence remains
in the cloud workspace. These artifacts exclude the executable itself.
Final combined Pi comparison, Web/TUI/protocol and recovery validation remain
required. This comparison excludes the later integrity/RawValue correction,
atomic settlement and partial-coalescer changes.

Reproduction from the frozen lazy worktree (new output directories required):

```sh
python3 benchmarks/runtime/build-revision.py --repo /workspace/rustX-pi-lazy --target /workspace/issue-481-target --output /workspace/issue-481-evidence/pi-lazy-build
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/pi-index-build/conformance --after /workspace/issue-481-evidence/pi-lazy-build/conformance --output /workspace/issue-481-evidence/pi-lazy-matrix --pairs 3
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-lazy-matrix
python3 /workspace/issue-481-evidence/pi-lazy-profile/run.py
python3 /workspace/issue-481-evidence/pi-lazy-profile/analyze.py
```
