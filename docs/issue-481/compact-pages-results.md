# Compact authenticated pages: isolated results

Production `7ed053952b34705419e8b4ffffcc24ae3e423b0d` is compared directly with
certificate production `563279ac8b6aeac29b36e75f5682195df9dd6c0b`. This isolates
bounded fact sorting/spill and binary authenticated pages; metadata reuse and
append certificates are present on both sides. See [design and bounds](compact-pages.md).
All 120 formal samples pass: 36 runtime, 24 warm append, 24 cold append and 36 Pi.
The runtime and Pi matrices each validate 846 complete requests and 18 paired
core-request comparisons.

Every case/history has three paired, alternating fresh-process/store runs.
Ratios are medians of same-pair ratios, not ratios of unrelated medians. No slow
samples were removed. Formal runs did not overlap compilation, tests or profiles.
CPU includes product and driver at 10 ms resolution; memory is sampled RSS/PSS,
not peak; writes are process-attributed, not physical-device writes. Evidence
serialization/output follows cleanup counters. Native allocation counts are
unavailable. Whole-request context construction is not O(1).

## Runtime

| Case | Turn wall ratio | Turn CPU ratio | Lifecycle write ratio | Sampled RSS ratio | Sampled anonymous PSS ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 0.9483 | 0.9628 | 0.9674 | 0.9994 | 0.9982 |
| short-large | 0.9104 | 0.9014 | 0.9547 | 0.9999 | 0.9984 |
| large | 0.9662 | 0.9583 | 0.9604 | 0.9964 | 0.9914 |
| stream | 1.0013 | 1.0417 | 0.9106 | 0.9965 | 0.9932 |
| tool | 0.9390 | 1.0000 | 0.8752 | 1.0034 | 1.0012 |
| cancel | 0.9233 | 1.0000 | 0.8846 | 1.0061 | 0.9972 |

Short-large wall/CPU improve 9.0%/9.9% in this matrix; short-small improves
5.2%/3.7%. Lifecycle writes improve 4.5%/3.3% respectively. Streaming wall is
flat and its CPU is 4.2% adverse. Memory is essentially flat; small adverse RSS
observations are retained. This is not a universal latency or memory improvement.

## Marginal append controls

| Mode | History | Append wall ratio | Append CPU ratio | Marginal write ratio | Lifecycle write ratio |
|---|---:|---:|---:|---:|---:|
| warm | 0 | 0.9885 | 1.0000 | 1.0000 | 0.9994 |
| warm | 64 | 0.9882 | 1.5000 | 1.0000 | 0.9998 |
| warm | 512 | 0.9868 | 0.6667 | 1.0000 | 1.0000 |
| warm | 2048 | 0.9922 | 1.0000 | 1.0000 | 1.0000 |
| cold | 0 | 1.0069 | 1.0000 | 1.0000 | 1.0003 |
| cold | 64 | 0.9459 | 1.3333 | 1.0000 | 0.9998 |
| cold | 512 | 0.9936 | 1.8000 | 1.0000 | 1.0000 |
| cold | 2048 | 0.9846 | 0.6364 | 1.0000 | 1.0000 |

These Source-only changes do not remove more canonical append work. The controls
are mixed, including adverse cold-empty wall and coarse warm/cold CPU ratios.
Append-driver sampled RSS is 3.1–5.6% higher in this isolated comparison, while
sampled anonymous PSS is nearly flat. These are observed whole-process samples,
not allocation counts or proof of a heap regression. Complete inputs remain
visible rather than discarded as noise.

## Identity, validation and evidence

The candidate was built after root-crate cleanup, with fresh library and
conformance artifacts and the identical complete five-file benchmark fixture.
Candidate executable SHA-256:
`78e044bee701759c17884e2b3813c8f10990ee4e92996008b6a8e65ef243c223`.
Certificate SHA-256:
`a9c7e712c54340671af0f6387920e259df754ec26659bb398bdafdb433d030d5`.
The independently clean-rebuilt metadata baseline confirmation remains in the
[identity audit](evidence/build-identity-audit.json).

393 durable tests, 21 focused tests, all-target/all-feature Clippy, formatting,
37 Python harness tests and eight Pi oracle tests pass. Initial development
failures are retained. Exact evidence-head FULL/Merge status is recorded in
[PR #484](https://github.com/Caismis/rustX/pull/484), rather than inferred from
an earlier production or documentation head.

[Plain pairs](evidence/pair-inputs), [lifecycle/footprint details](evidence/final-lifecycle-details.json),
[separate profiles](final-structural-profiles.md), and the [verified compact archive](evidence/final-structural.tar.xz)
with its [manifest](evidence/final-structural.tar.manifest.json) preserve all inputs,
commands, build identities, logs, process outcomes and complete oracles. Original
files and immutable executables remain in the cloud workspace.
