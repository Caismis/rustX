# Native query-plan stability: isolated comparison

Measured after: `15e7e7125dc34f54a245ea06e95f517d67c9ea53`, executable SHA256 `a247a8b644e8a9b06956548aecd6a78f63129220ed4d09dd04903d433e91c919`. Before: retained final six-change production `a3c116fe91d7689d86c5009c7b563289d3cd34cc`, SHA256 `e99efcdbcb608b94e2868b5b5c2f48e6d9c4d67f631e18a602ec6b7f0e1a349a`. Dependency remains #484 `7d659c06f692277d4711c54b579f9fe0f8d8792d`; verified main `3c355f152d405a201482d53b6e45497df4f44a7f`. No Source navigation change is included.

The only production change enables SQLite query-plan stability on native connection owners. It prevents parameter changes from recompiling indexed plans; schema changes still invalidate plans. No persisted format, publication cadence, authority, error handling or recovery contract changes. SQLite documents a tradeoff: disabling parameter-sensitive optimizations can make some queries slower. The bounded-index tests and complete workloads cover this change; no universal speed guarantee is claimed.

## Runtime comparison

Three interleaved pairs per case, new processes/stores, same optimized build settings and sibling local HTTP/SSE provider: **36/36 samples, 846 full ordered requests and 18 pair comparisons passed**. No builds, tests, profilers or compression overlapped formal samples. Values below are medians of paired after/before ratios, not ratios of unrelated medians. No new Pi-equivalence claim.

| Case | Turn wall | Turn CPU | Lifecycle CPU | Lifecycle writes | Sampled anonymous PSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.8826 | 0.8979 | 0.9345 | 1.0001 | 1.0080 |
| short-large | 0.8871 | 0.8885 | 0.9529 | 0.9992 | 1.0077 |
| stream | 0.9968 | 1.0000 | 0.9412 | 0.9982 | 1.0001 |
| large | 0.9614 | 1.1429 | 1.0455 | 1.0000 | 1.0078 |
| tool | 0.9269 | 1.0000 | 0.9630 | 0.9993 | 0.9977 |
| cancel | 1.0120 | 0.8571 | 0.9524 | 1.0000 | 1.0061 |

Short-turn median wall/CPU reductions are provisionally about 11–12%/10–11%; writes are effectively unchanged. Short-small intervals cross 1, so these exploratory medians do not establish a stable universal gain. Adverse samples remain visible: short-small pair 1 CPU is 86.25 vs 84.6875 ms/turn, cancellation pair 0 wall is 59.076 vs 43.179 ms, and large-case median paired CPU ratio is 1.1429. Large-case CPU samples are only 6–8 clock ticks: the increase is unresolved at this sample size, not erased as noise. Sampled anonymous memory increases about 0.8% in both short cases.

## Uncertainty

The intervals below are 90% paired log-ratio t intervals (their upper endpoints are one-sided 95% limits; df=2). They describe geometric means, separately from the paired medians above, and assume approximately normal independent log ratios. With only three pairs they are exploratory. Final Pi comparability uses the [predeclared seven-pair policy](comparison-policy.md), all cases and all cost dimensions.

| Case | Wall geometric mean [interval] | CPU geometric mean [interval] |
| --- | ---: | ---: |
| short-small | 0.917 [0.817, 1.029] | 0.932 [0.818, 1.062] |
| short-large | 0.890 [0.813, 0.973] | 0.887 [0.790, 0.995] |
| stream | 0.995 [0.984, 1.005] | 0.950 [0.818, 1.104] |
| large | 0.958 [0.873, 1.052] | 1.046 [0.782, 1.398] |
| tool | 0.936 [0.904, 0.969] | 0.969 [0.883, 1.063] |
| cancel | 1.072 [0.739, 1.555] | 0.984 [0.631, 1.534] |

## Marginal canonical append

**48/48 warm/cold samples passed**, 32 measured appends per sample at histories 0/64/512/2048, separately from model context construction. Cold means reopening the native connection before each append, not clearing kernel caches. Paired median after/before ratios:

| History | Warm wall | Warm CPU | Cold wall | Cold CPU | Writes, warm/cold |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 0.9628 | 1.5000 | 0.9740 | 1.0000 | 1.0000/1.0000 |
| 64 | 0.9987 | 1.0000 | 0.9645 | 0.4286 | 1.0000/1.0000 |
| 512 | 0.9437 | 1.5000 | 0.9822 | 0.8000 | 1.0000/1.0000 |
| 2048 | 0.9725 | 1.0000 | 0.9166 | 0.6000 | 1.0000/1.0000 |

Warm CPU regressions at histories 0 and 512 are retained. Whole 32-append CPU intervals there contain only 2–3 ticks; per-append values 0.625/0.9375 ms reflect that coarse resolution. Cold CPU uses per-operation brackets including their counter overhead, with reopen costs separately retained in each summary. Wall, CPU and writes have distinct scopes; no whole-request O(1) or pure DB-cost claim.

## Reproduction and validation

[Readable per-pair inputs, CSV and independent recomputation](evidence/query-plan/readable/README.md) expose all 84 samples. All 594 numeric paired ratios, all medians and all 12 runtime intervals reproduce exactly; raw hashes and raw-to-summary derivation also pass. Parameter-sensitive plan tradeoffs remain: six workloads and four append histories do not cover arbitrary production data distributions or larger scales.

`run-measurements.py` records the compiler/executable checks, host/quota identity and exact commands. It runs `compare-revisions.py` with three pairs across all six cases, then `compare-append.py` with three pairs at all four histories, warm and `--cold`. Summaries and every original sample are in the [verified evidence archive](evidence/query-plan-results.tar.xz) and [manifest](evidence/query-plan-results.tar.manifest.json). The readable uncertainty file is reproduced by the archived report script. Original evidence is retained.

CPU is self user+system across product threads plus driver, at 10 ms resolution; the provider/controller are separate siblings and provider teardown follows product cleanup. Runtime lifecycle includes verification, checkpoint, close, reopen and cleanup; evidence-file output is afterward. Startup/control and all lifecycle phases remain in raw results. Writes are process-attributed, not physical-device writes. RSS/PSS are sampled maxima, not peaks. Allocations remain unavailable. No overhead subtraction or multiplied stage ratios.

Correctness: 56 distinct native storage tests (including differential historical query rows, zero recompiles and no full scans/sorts), 18 Python controller negatives and 8 Pi oracle negatives pass; formatting/whitespace pass. Test-only compilation used root-package opt-level=0, while both measured executables use the identical normal optimized release profile. FULL for measured head: [39177505760](https://github.com/Caismis/rustX/actions/runs/39177505760), cancelled after later pushes; no exact-head FULL success is claimed. No merge.
