# Partial publication cadence

The default oldest-payload deadline is 100 ms. This triggers a flush decision;
synchronous persistence and scheduling can add delay, so it is not a delivery SLA. Small responses that complete
before that deadline need no intermediate partial transaction. Tool proposal
start/completion remain distinct ordered frames but share the same commit batch.
The caller remains the single synchronous persistence writer, commits before
releasing frames, and drains final frames synchronously. There is no background
queue or separate precommit ContentRef authority and no pending write can race
past finalization. Cancellation/provider failure retains the current synchronous
audit drain; process crash retains only previously committed partials.

This is not identical to Pi's timer-only full-message snapshots. Rust also flushes
at 64 KiB conservative payload weight or 128 nonmergeable frames to bound pending
work. One incoming provider payload may exceed that threshold; it is flushed
without truncation. Text, tool identities and completed arguments contribute to
pressure. Argument sizing uses a bounded counting writer, not another JSON buffer.
String capacities and JSON container allocations are not measured allocation
counts; these are policy bounds, not a claimed exact heap-byte limit.

The injected-clock tests cover 99/100 ms, unchanged oldest deadlines, pressure
before the timer, empty payload frame limits, oversized completed tool arguments,
exact preserved order/content and terminal/reset fencing. The client also installs missing proposal facts from the existing bounded
canonical message descriptors before rebinding their native locators. A final
tool proposal therefore remains visible when no intermediate frame was published;
execution state still comes from native execution journal facts. No extra SQL or
persistence owner is introduced.

Validation: 10 coalescer tests, 606 scripted tests, 89 tests matching Agent
execution, 82 projection tests, 38 publication tests, and 70 process-death tests
passed, plus all-target/all-feature Clippy with warnings denied. These filters
overlap and are not an additive unique-test count. The process suite used the
freshly rebuilt product binaries and required the local fake provider. A mistaken
integration-binary invocation selected zero tests; its log is retained and is not
counted. The concurrent mailbox probe verifies publication retains its lease
through terminal notification while a producer attempts admission.

Initial failures are retained: stale frame-only client expectations, a real gap
in canonical-only tool proposal events, fixture UUID version, style lints, and
an ENOSPC link failure before tests ran. Only regenerable build caches were
removed to recover space; original evidence and benchmark executables remain.
Formal isolated/combined measurements and exact final-head FULL/Merge CI remain
pending. See evidence/partial-validation for raw checks and a hash manifest.

FULL follow-up found two stale external fixtures, both retained in the validation
archive: the Source point-query still named the removed witness column, and the
three-protocol streamed-turn test expected all reasoning only as inline deltas.
The point-query now names checksum with identical point-index/no-scan assertions.
The protocol test requires one exact canonical client answer, exact reasoning,
equality with the native original body, any partial as an exact prefix, and the
unchanged exactly-once/last terminal checks. Its local optimized test passed all
three production protocols in 2.95 s with the mandatory fake provider. Production
code and the preserved cadence benchmark executable are unchanged by these fixes.

## Isolated cadence measurement

Atomic predecessor `ce8f7270ef65ef279f9c5d6c73135b420074dab0` versus cadence
`a3c116fe91d7689d86c5009c7b563289d3cd34cc`, source tree
`be0a92467809fd493aad64e8b1dc6b5e0d7a9f61`. The preserved cadence executable
SHA-256 is `e99efcdbcb608b94e2868b5b5c2f48e6d9c4d67f631e18a602ec6b7f0e1a349a`.
Clean release build identities verify the same compiler, flags and fixture hashes.
The later test/document fixes do not change production or this preserved binary.

All 36 fresh samples, 846 ordered requests and 18 paired comparisons passed.
Medians below are three same-pair ratios, not ratios of unrelated medians.

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9926 | 1.0075 | 1.0006 | 1.0187 | 1.0105 | 0.9980 |
| short-large | 1.0261 | 1.0171 | 1.0174 | 1.0156 | 0.9942 | 1.0007 |
| stream | 0.9971 | 0.9091 | 1.0025 | 0.9730 | 0.9967 | 0.9114 |
| large | 0.2696 | 0.3182 | 0.6300 | 0.5946 | 0.9869 | 0.3362 |
| tool | 0.9708 | 1.0909 | 0.9674 | 0.9615 | 1.0046 | 0.9869 |
| cancel | 1.0362 | 1.0000 | 1.0036 | 1.0000 | 1.0009 | 0.9980 |

Large-output wall/CPU improve about 73.0%/68.2%, with lifecycle writes down
66.4%. Paced-stream writes fall 8.9%; its wall remains mostly provider pacing.
Short turns establish no general gain: short-large wall/CPU regress 2.6%/1.7%,
tool CPU regresses 9.1%, and cancellation wall regresses 3.6%. Short-small
anonymous PSS rises about 1.0%. Three pairs are limited evidence, not confidence
intervals, and all adverse samples are retained.

The corrected large-output diagnostic records 145 → 17 WAL write commits,
2,302 → 405 frames and 9,484,240 → 1,668,600 frame bytes for one measured turn.
This corroborates fewer partial persistence transactions. The short-small
diagnostic has 542 → 543 write commits, 12,794 → 12,780 frames, and
52,711,280 → 52,653,600 frame bytes over 32 turns. That tiny difference is not
attributed solely to layout or cadence, especially with differing commit counts.
Endpoint dbstat ownership does not exclude intermediate page reuse.

Two initial diagnostic profiles accidentally selected the cadence executable on
both sides through chained path substitution. They are explicitly withdrawn:
`pi-partial-profile/WITHDRAWN.json` and
`pi-partial-large-profile/WITHDRAWN.json`. Their original files remain archived.
The formal timing matrix used distinct identity-verified executables throughout
and is unaffected. Corrected profiles use explicit before/after paths and a
distinct-binary guard; both identities were additionally checked against the
formal build SHAs. The stage controller now verifies each profile side against
its formal counterpart, with the withdrawn A/A case as a negative check.

CPU is product/driver self CPU at 10 ms resolution; provider and controller are
excluded. RSS/PSS are sampled maxima, not peaks; allocations are unavailable.
Writes are process-attributed, not device writes. Lifecycle includes checkpoint,
verification, close/reopen and cleanup; evidence output follows counter capture.
Provider start/control and complete controller wall remain explicit in raw
records. Profiles ran separately from timings; no build/test/profile/compression
overlap occurred during formal samples. Source temp files live on tmpfs; neither
WAL frame bytes nor process writes imply physical-device savings.

The stage comparison is also final-minus-cadence: it includes the necessary
canonical-only tool projection repair. Its benefit is workload-dependent and
comes with less frequent live publication and a larger uncommitted crash tail.
It remains synchronous commit-before-live with pressure exceptions; it is not
identical to Pi's timer-only persistence behavior.

Reproduce using the verified builder, then:

```sh
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/pi-atomic-build/conformance --after /workspace/issue-481-evidence/pi-partial-build/conformance --output /workspace/issue-481-evidence/pi-partial-matrix
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-partial-matrix
python3 /workspace/issue-481-evidence/pi-partial-profile-corrected/run.py
python3 /workspace/issue-481-evidence/pi-partial-profile-corrected/analyze.py
python3 /workspace/issue-481-evidence/pi-partial-large-profile-corrected/run.py
python3 /workspace/issue-481-evidence/pi-partial-large-profile-corrected/analyze.py
```

[Raw archive](evidence/partial-measurements/measurements.tar.xz),
[manifest](evidence/partial-measurements/measurements.tar.manifest.json), and
[formal summary](evidence/partial-measurements/formal-summary.json) preserve the
complete data, identities, corrected/withdrawn profiles and reproducible scripts.
Final combined/leave-out evidence and FULL verification remain separate gates.
