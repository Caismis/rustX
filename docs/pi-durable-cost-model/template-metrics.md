# Stage 1: reuse new-leaf template metrics

A new leaf used to load/authenticate/decode its template for each of 80 width
classes, encode the same base provenance hole 80 times, then load each template
again for path bounds. The matched #484 diagnostic found 64 single-template new
leaves across 32 steady turns: 5,184 loads in these two passes. Existing native
leaves already reuse their stored augmentation and do not repeat this work.

`nodes::leaf_geometry` now loads each capture-deduplicated template once, derives
its one-digit base width and maximum path bounds, then builds the same 80 widths
by adding the established class delta. It retains only at most 80 `u64` base
widths, not decoded templates or an identity cache. The bounded template bytes
are still checked and hashed before decoding the same buffer. Error handling and
persisted encoding are unchanged; this stage changes no format or guarantee.

Test-only counters distinguish template loads, new-leaf geometry evaluations and
base-hole encodes. Single- and three-template fixtures exercise all classes,
including maximum native u64 provenance lengths; malformed references, width
overflow and changed authenticated bytes remain rejected. A real RequestStart
fixture checks legacy full-byte/field/navigation oracles, then captures the same
native leaves again and requires zero new geometry/hole work. Existing durable
Source historical, witness, cold-bound, recovery and Fork tests remain in scope.

The operation reduction is not a wall-time claim. Stage measurements use a clean,
identity-verified release executable and the unchanged local HTTP/SSE provider
against #484's preserved production binary. The measured performance results and
validation outcomes are recorded separately once complete.

Validation: `cargo test --locked --lib durable::sqlite:: -- --nocapture` passed
398 tests (302.01 s); all-target/all-feature Clippy with warnings denied and
formatting/whitespace checks passed. The initial missing `Template` import was
corrected; its failed compile log remains in the cloud evidence directory.

## Measured stage result

Three paired, interleaved fresh-process runs per case (12 samples total) compare
#484 production `7ed053952b34705419e8b4ffffcc24ae3e423b0d` to
`989d74d12d211a38eee5ad1f07ec18c0251a0de5`. Both executables have verified clean
root-library rebuilds, distinct SHA-256 hashes, and identical five benchmark
fixture hashes. Every full ordered oracle and pair comparison passed: 816
requests in total (40 per short-small sample, 96 per short-large sample).

| Metric: median of paired after/before ratios | short-small | short-large |
|---|---:|---:|
| Mean steady-turn wall | 1.01123 | 1.01642 |
| Self CPU per steady turn | 1.01613 | 1.01563 |
| Complete lifecycle process-attributed writes | 0.99937 | 1.00005 |
| Maximum sampled anonymous PSS | 0.98828 | 0.99857 |

This is **not a demonstrated runtime speedup**. Wall and CPU are slightly worse
in this sample; none are discarded. The operation reduction is real but small
relative to the event/index and transaction work identified in the baseline.
Memory figures are sampled maxima, not peaks or allocation counts. CPU includes
the runtime and driver, has 10 ms process-tick resolution, and excludes the
sibling provider. Lifecycle includes checkpoint, close, reopen and cleanup;
verification/evidence output follows product counter collection. Provider startup,
control and teardown remain recorded separately. Physical-device writes and
allocation counts are unavailable. No timings from traced diagnostics enter this
comparison. Hardware/build/runtime conditions follow the #484 cloud baseline.

Reproduce from the frozen stage checkout:

```sh
python3 benchmarks/runtime/build-revision.py --repo "$PWD" --target /workspace/issue-481-target --output /workspace/issue-481-evidence/pi-template-build
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/compact-verified-build/conformance --after /workspace/issue-481-evidence/pi-template-build/conformance --output /workspace/issue-481-evidence/pi-template-matrix --pairs 3 --case short-small --case short-large
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-template-matrix
```

The [plain summary](evidence/template-summary.json) retains each pair and phase;
the [verified compact archive](evidence/template-metrics.tar.xz) and
[manifest](evidence/template-metrics.tar.manifest.json) retain raw samples,
provider/controller output, build identities and validation logs. Cloud originals
remain in `/workspace/issue-481-evidence`; no input was destroyed.
