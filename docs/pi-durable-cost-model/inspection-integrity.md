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
original meaning and labels. New profile and formal comparisons remain pending; no performance gain is claimed.
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
