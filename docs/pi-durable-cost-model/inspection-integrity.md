# Request Source integrity without independent capture proofs

This stage removes the duplicate root witness row and the origin-source/rank
proof walk for reused Source nodes. Native selected-input identity, request
sequence, occurrence order, historical cut and ownership remain authoritative.
The existing content frontier fences the root extent; it is neither a new row
nor an independent byte witness. Existing immutable page/blob hashes remain for content addressing and bounded
corruption checks; the root has one checksum. A checksum is not an independent
original-capture witness. A coherently rewritten optional Source graph can pass
these checks, and the negative tests explicitly demonstrate that revised limit.
Canonical input and frozen request selection remain separate native authorities.

There is no replacement witness, recapture, audit truncation or persistence queue.
Source byte ranges, field paths, previews and pagination retain their interfaces.
Malformed lengths, depth, native identity, page encoding and inconsistent hashes
still fail. Optional indexing stays within its fail-open savepoint, so failure
cannot roll back a healthy native request commit. Artifact seals and same-buffer
copy/archive verification are outside this change and remain intact.

The consumer audit covers request field/range/page dispatch, typed metadata,
Trace tool definitions, schema/FK checks, fail-open insertion tests, scalar request
finalization, historical cuts and reused node selection. The redundant table was
only a root seat; all request field operations already dispatch to the composed
Source reader. No protocol JSON field is added or removed. SQLite gate 74 rejects
previous layouts without migration, fallback or dual writes.

The diagnostic profiler now calls this operation a node lookup, not capture
authentication. Original-proof path/cache counters are removed rather than
reported as successful proof work. Existing historical measurements retain their
original meaning and labels. The isolated comparison below shows small write savings and mixed CPU/memory results; no general performance gain is claimed.
Validation passed: all-target/all-feature Clippy with warnings denied, all 407
durable SQLite tests (253.37 seconds), and all 26 Source tests (1.05 seconds).
The six lazy-preview fixtures and three corrected integrity/reuse checks also
passed separately. `evidence/inspection-integrity/` preserves successful checks
and initial failures with a manifest, including the whitespace-parser finding,
two stale renamed-column mutations, the superseded old-root dependency assertion
and the test-only Scalar variant compile correction.

The independent review's additional RawValue/duplicate-key fixtures exposed a
preexisting compact-JSON-only parser assumption. Source construction now scans
JSON whitespace without normalizing original bytes; token ranges remain exact.
Duplicate reserved keys retain ordinal navigation. Ordinary duplicate named
paths retain the existing fail-open optional-index refusal, with original generic
Source bytes and historical sources intact. This increment includes that lexical
correctness fix; its eventual performance comparison must disclose both changes.

With `issue-387-profile`, `RUSTX_SOURCE_PROFILE=1` enables scalar timing and
operation diagnostics. `lookup_calls` counts native-key hash lookups and
`source_node_loads` counts full augmentation loads; neither claims to count all
SQLite bytes or device I/O. Phase elapsed timings overlap and are not CPU shares.
The previous authentication-specific environment variable and proof counters are
removed, not emitted as successful zero-work authentication.

## Isolated cloud comparison

Compared lazy previews `b5840bd903d2508e693e1a4f80c3dc6173c9e29b`
(binary `e6e3c0109de5202820201b1aa657f2e018223240934131829cb7e5761936a474`)
with integrity/RawValue `5e37b9240290723483ac00855cee7eee58760c81`
(binary `a12ce8c51d38264328ff06658d7ab971591a6dbb0fba1d752f498b3b936c0f54`,
production tree `0873f5f136f276eb109f5de832a69d319932a41a`). The optimized build
finished at 14:28:14 UTC on 2026-10-18. Fresh root artifacts, identical compiler,
build settings and all five benchmark fixture hashes were verified. No compiler,
test run or profiler overlapped formal timing.

All 36 fresh-process samples, 846 full ordered requests and 18 paired comparisons
passed. There are three interleaved pairs per workload; every sample is retained.
The following ratios are medians of within-pair after/before ratios:

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9737 | 1.0204 | 0.9743 | 1.0151 | 1.0096 | 0.9977 |
| short-large | 0.9444 | 0.9547 | 0.9718 | 0.9723 | 0.9982 | 0.9973 |
| stream | 0.9996 | 0.9048 | 0.9915 | 0.9211 | 1.0031 | 0.9955 |
| large | 0.9260 | 1.0909 | 0.9613 | 1.0270 | 1.0062 | 0.9981 |
| tool | 0.9793 | 1.0000 | 0.9743 | 1.0000 | 1.0193 | 0.9941 |
| cancel | 1.0044 | 1.0000 | 0.9899 | 1.0000 | 1.0007 | 0.9949 |

Lifecycle process-write savings are approximately 0.2–0.6%. This is not a general
CPU or memory improvement: short-small turn CPU increases 2.0%, large turn CPU
increases 9.1%, and tool sampled anonymous PSS increases 1.9%. Short-large turn
wall/CPU improve 5.6%/4.5%, while cancel wall is slightly worse. Small CPU totals
are coarse at 10 ms self-CPU resolution. These mixed results are weak support
for the meaningful loss of independent original-capture authentication. The user
will choose retained scope after final combined and leave-one-out evidence; no
automatic reversal is performed. A reversal must retain the RawValue correctness
repair and differential fixtures.

A separate instrumented 32-turn diagnostic has exactly 672 WAL write-commit
markers and 256 new events on each side. WAL frames fall from 16,355 to 16,348
(67,382,600 to 67,353,760 frame bytes). The previous 32 redundant root-seat rows
are absent. Endpoint dbstat attribution cannot exclude intermediate page reuse;
complete traces, mixed/unmapped ownership and snapshot overhead are retained.
This profile is outside formal timing and supplies no exclusive CPU percentage.

CPU includes the runtime and measurement driver, not the sibling fake provider.
Provider startup/control/cleanup costs are retained separately; the external
controller keeps the provider alive through product cleanup. Lifecycle includes
verification, checkpoint/close/reopen and cleanup, with evidence-file output
after counter capture. RSS/PSS values are sampled maxima, not peaks; allocations
are unavailable. Process-attributed writes and WAL frame bytes are not physical
device writes. No unrelated medians are subtracted. DB/WAL logical and allocated
footprints and phase counters remain in each complete sample. Cold physical-read
bounds retain their tests; this comparison is not a first-read latency benchmark.

[Complete evidence archive](evidence/integrity-measurements/measurements.tar.xz),
[manifest](evidence/integrity-measurements/measurements.tar.manifest.json),
[formal summary](evidence/integrity-measurements/formal-summary.json), and
[WAL diagnostic](evidence/integrity-measurements/wal-summary.json) preserve all
records, scripts and build identities; originals and executables remain in the
cloud workspace. Atomic settlement and partial cadence are excluded.

```sh
python3 benchmarks/runtime/build-revision.py --repo /workspace/rustX-pi-integrity --target /workspace/issue-481-target --output /workspace/issue-481-evidence/pi-integrity-build
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/pi-lazy-build/conformance --after /workspace/issue-481-evidence/pi-integrity-build/conformance --output /workspace/issue-481-evidence/pi-integrity-matrix --pairs 3
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-integrity-matrix
python3 /workspace/issue-481-evidence/pi-integrity-profile/run.py
python3 /workspace/issue-481-evidence/pi-integrity-profile/analyze.py
```

## Independently recoverable lexical repair

The [standalone correctness patch](evidence/rawvalue-correctness/correctness.patch)
and [manifest](evidence/rawvalue-correctness/manifest.json) extract only the
whitespace parser repair and RawValue/duplicate-key differential fixtures from
`5e37b924`. `git apply --check` passed on frozen `b5840bd9`. The patch excludes
integrity schema and gate changes. This preserves an independent repair artifact
without rewriting reviewed history; final authentication-restored variants must
retain and validate it. It is not a separately measured executable.

Independent review P2 identified a missing generic helper in the original artifact.
The corrected patch includes that helper and historical-source assertion, applies
to `b5840bd9`, and was compiled/tested on the actual resulting baseline tree
`ac9042ea2b72c73c2ea7b27f16e590d8eb8d8b18`: **6 preview and 26 read-layout
tests pass**, with schema gate 73 unchanged. Full compiler/command identity and
readable logs are in the linked manifest. The original incomplete artifact is
preserved in history and the validation archive; this does not alter production.
