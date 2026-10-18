# Continued optimization stages

These stages follow the attribution checkpoint in `attribution.md`. They retain
all slow samples and use unprofiled optimized executables, clean processes and
stores, the same controller-owned sibling HTTP/SSE provider, and three
interleaved matched pairs for each of the six workloads. No build, test,
profiling or compression overlapped these formal samples. Performance ratios
are medians of matched-pair ratios; memory is sampled, not peak.

## Observation reservations

Production commit `be33d09b` reserves only the possible bounded work under the
existing queue lock. Transition admission, ordering, source release, wakeups,
publication cadence and the 32/33 item allocation bounds remain unchanged.
The same-sequence proof count scans at most 32 already queued values; it performs
no Source read, callback or I/O. Thirteen observation tests pass, including
explicit short-page, bare-witness and idle-drain allocation bounds.

Before executable SHA-256:
`b5c58303ec9960e67afe75dd23548de7e82d90aeed511f7c9953daa0235f8345`.
After executable SHA-256:
`078fe0e37433cbe797f942d52313530ef4d452271a84e5d7d0083110e8219a00`.
Both contain the same diagnostic phase hooks, disabled in these samples.
All 36 samples and full ordered request comparisons passed.

| Workload | Wall ratio | CPU ratio | Sampled RSS ratio | Lifecycle process-write ratio |
|---|---:|---:|---:|---:|
| short-small | 1.0298 | 1.0163 | 1.0038 | 0.9999 |
| short-large | 1.0359 | 1.0501 | 1.0033 | 0.9995 |
| stream | 0.9973 | 1.0000 | 1.0204 | 1.0000 |
| large | 0.9984 | 1.0435 | 1.0017 | 0.9997 |
| tool | 0.9942 | 0.9286 | 1.0126 | 0.9987 |
| cancel | 1.2926 | 1.1429 | 1.0161 | 1.0017 |

This is **not a demonstrated speedup**. Cancellation and other adverse samples
remain included; CPU for single-turn cases has coarse 10 ms resolution.
The original six-pair 1.3334 canonical wall regression is not replaced by these
new three-pair observations.

At the after-turns endpoint, median paired anonymous PSS fell 936 KiB for
short-small and 1,520 KiB for short-large. File-backed PSS rose 2,600/2,488 KiB,
and total RSS rose 1,376/968 KiB at that endpoint. These component medians need
not add. This is an anonymous-memory tradeoff, not a reduction in total sampled
RSS or proof of an allocation-attribution cause. The attribution profile's
77 MiB reservation traffic motivated the change; post-change allocation
profiling remains separate from these formal measurements.

`evidence/reservations.tar.xz` and its verified manifest retain every raw sample,
comparison, process status, test/build log and summary. Originals remain under
`/workspace/issue-481-evidence`. Further stages, fresh Pi controls, combined
comparison and final exact-head FULL/independent review remain outstanding.
