# Review follow-up

The original six-pair canonical-incremental short-small wall ratio **1.3334**
remains a regression in the headline evidence. The diagnostic runs below do not
replace it or establish its cause. No Rust production code changed during this
follow-up. Performance acceptance and independent review remain outstanding.

## Current harness coverage

The previously reported nine Python tests exercised the historical v2 harness,
not the new HTTP controller. That description is corrected. Fourteen new
deterministic tests exercise the current controller and summaries: sibling
ownership, provider lifetime through product cleanup/exit, rejected and failed
runs, ordered unique lifecycle endpoints, self CPU excluding child ticks,
seed-excluding append counters, unavailable metrics, matched-pair arithmetic,
and missing, duplicate, rejected or incompatible pairs. These use controlled
process/clock fixtures, not sleeps or retries. All **23 Python tests** and the
59 CI-script tests pass; CI now runs the runtime Python suite explicitly.

The stricter lifecycle validator reaccepted all **252 runtime samples / 5,922
requests**. Recomputed original headline median fields are unchanged. Small
[plain JSON inputs](evidence/pair-inputs/README.md) expose every pair, including
[all six canonical short-small pairs](evidence/pair-inputs/canonical-runtime-short-small.json).
Their raw sample/status hashes were verified against the retained archive
manifests. The original nine tests are labelled legacy coverage, not evidence
for the new controller.

## Wall regression diagnosis

In original confirmation pair 4, before/after mean turns were 69.64/107.27 ms.
The after run had six late turns around 164–384 ms, while its before run stayed
around 60–84 ms. CPU for those slow turns remained around 90–120 ms, comparable
to ordinary turns. Extra elapsed time appears both before first publication and
between publication and settlement. This localizes the regression to bursts of
waiting rather than demonstrating consistently greater CPU work; it does not
identify the waiting cause. No major faults were counted. Block-I/O delay ticks
were zero, but task delay accounting is currently disabled, so zeros do not
establish absence of I/O waits. The original runs did not record cgroup
throttling or scheduler pressure, and those facts cannot be reconstructed.

Additional diagnostics used the same immutable Source-only and combined
executables, same production adapters/provider and clean processes/stores.
An external 10 ms sampler recorded task wait channels, scheduler accounting,
cgroup counters and dirty pages. Each group contains three interleaved pairs;
all 18 samples and their full request comparisons passed and are retained.

| Diagnostic condition | Median matched after/before wall ratio |
|---|---:|
| Workspace overlay stores and evidence | 0.9933 |
| tmpfs stores and evidence | 1.0612 |
| Workspace, both executables read before each process | 0.9612 |

The filesystem factor includes evidence output and background writeback, not
just database storage. Executable pre-reading cost roughly 16–18 ms per run,
outside product timing. External sampling, and mapping inspection in the warm
runs, can perturb scheduling; these are diagnostic observations, not additional
headline performance samples. No cache dropping or system setting changes were
made. The newly observed cgroup limit is four CPU equivalents despite five
visible CPUs; no throttled time accrued in any diagnostic run. This is not a
retroactive assertion about the original runs. Workspace runs sampled storage
wait channels such as `folio_wait_bit_common` and `submit_bio_wait`; tmpfs runs
did not. Sample counts are not elapsed or CPU percentages. Neither observation
proves the cause of the original slowdown. No corresponding large stall burst
recurred, including with warm executables.

## Memory diagnosis

These are medians of **paired endpoint deltas**, in KiB, from the original
headline inputs. They are not differences of unrelated medians or peak claims;
columns need not add because their medians are computed independently.

| Comparison and endpoint | RSS delta | Anonymous PSS delta | File PSS delta |
|---|---:|---:|---:|
| Source, after initialization | 296 | -64 | 362 |
| Source, after measured turns | 1,272 | 864 | 412 |
| Canonical, after initialization | 3,212 | 4 | 3,190 |
| Canonical, after measured turns | 2,406 | 466 | 1,894 |
| Combined, after measured turns | 2,348 | -4 | 2,468 |
| Append at 2,048, after seed | 3,454 | -162 | 3,616 |
| Append at 2,048, after appends | 3,342 | -274 | 3,616 |

Canonical before/after use the same Source implementation. Most of that
comparison's RSS increase is file-backed and exists before measured turns;
it cannot be assigned to a new Source retention change. Diagnostic mapping
samples identify variable executable residency: warm paired maximum executable
PSS was 58,116/60,220, 58,172/60,156 and 58,376/56,976 KiB. This supports a
file-residency contribution without establishing a complete causal allocation
profile. Source-only has a real anonymous-memory tradeoff consistent with
retained small indexes/allocator state. Allocation counts and exclusive heap
attribution remain unavailable. No memory-reduction claim is made.

## Reproducibility

[Plain analysis](evidence/review-analysis.json) retains all diagnostic paired
turn arrays, hashes, sampled wait channels and memory endpoint deltas.
`evidence/review.tar.xz` and its manifest preserve raw diagnostic samples,
controller/provider logs, revalidation and test logs, and the analysis script.
Originals remain in `/workspace/issue-481-evidence`; original tmpfs diagnostics
also remain in `/tmp/issue-481-contention-tmpfs`.

From the repository root, with the immutable binaries recorded in the manifests:

```sh
python3 -m unittest discover -s benchmarks/runtime -p 'test_*.py' -v
python3 benchmarks/runtime/profile-contention.py \
  --before /workspace/issue-481-evidence/source-append-conformance \
  --after /workspace/issue-481-evidence/combined-conformance \
  --workspace-output /workspace/issue-481-evidence/new-contention \
  --tmpfs-output /tmp/new-contention
```

Use fresh output directories. Add `--warm-executables --only-workspace` for the
mapping/pre-reading diagnostic. Run separately from builds, tests and formal
benchmarks. The historical executable hashes are recorded in each manifest;
new harness tests and stricter summaries do not change those executables.
