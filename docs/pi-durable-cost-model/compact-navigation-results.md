# Compact Source navigation: isolated comparison

After: `97dfdca84335f6f5350c78356ab04ace1d1c11fe`, optimized executable SHA256 `6cbd296813fe7fa8fee9d9d2be58fef390e4616aabadd850d731c59017cc205f`. Before: query-plan stage `15e7e7125dc34f54a245ea06e95f517d67c9ea53`, executable `a247a8b644e8a9b06956548aecd6a78f63129220ed4d09dd04903d433e91c919`. This isolates compact Source navigation; subscriber-aware display and lazy inspection are not included.

Three interleaved pairs across six cases: **36/36 samples passed**, with the same production HTTP/SSE adapter, sibling fake provider, complete ordered request/result oracles and clean stores/processes. No compilation, tests, profiling or compression overlapped formal samples. This preserves the existing native workflow observation scope; it is not either of the newly declared aligned observer modes and makes no Pi parity claim.

| Case | Turn wall | Turn CPU | Lifecycle CPU | Lifecycle writes | Sampled anonymous PSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9670 | 0.9848 | 1.0000 | 1.0001 | 0.9985 |
| short-large | 0.9544 | 0.9609 | 0.9831 | 1.0000 | 1.0123 |
| stream | 0.9967 | 1.0000 | 1.0333 | 1.0000 | 1.0021 |
| large | 0.9390 | 1.0000 | 0.9091 | 0.9991 | 0.9997 |
| tool | 0.9853 | 0.9091 | 1.0000 | 1.0000 | 1.0081 |
| cancel | 0.9406 | 1.0000 | 1.0000 | 1.0010 | 0.9973 |

Values are medians of paired after/before ratios. Short-small wall/CPU reductions are provisionally 3.3%/1.5%, short-large 4.6%/3.9%. Writes are essentially unchanged. Short-large sampled anonymous PSS increases 1.2%, tool 0.8%; stream lifecycle CPU increases 3.3%. Every slow/adverse sample remains in the archive. Three pairs and 10 ms CPU ticks do not establish stable universal gains.

| Case | Wall geometric mean [90% interval] | CPU geometric mean [90% interval] |
| --- | ---: | ---: |
| short-small | 0.984 [0.918, 1.054] | 0.989 [0.916, 1.069] |
| short-large | 0.991 [0.881, 1.115] | 0.973 [0.930, 1.018] |
| stream | 0.995 [0.956, 1.034] | 1.038 [0.931, 1.156] |
| large | 0.952 [0.851, 1.064] | 1.000 [1.000, 1.000] |
| tool | 0.970 [0.922, 1.020] | 0.880 [0.791, 0.979] |
| cancel | 0.945 [0.801, 1.114] | 0.941 [0.788, 1.124] |

Intervals use the same predeclared paired log-ratio t method, df=2. They concern geometric means, separately from paired medians. Their upper endpoints are one-sided 95% limits. All metrics and per-pair ratios are preserved in `uncertainty.json`; assumptions and final seven-pair policy remain unchanged.

The prior counter profile found one 12–13 KiB SQLite-indexed request scalar per short turn. The new exact admission tests establish that this size now uses bounded compact navigation; eliminating that observed category of temporary index is an implementation inference, not a fresh instrumented workload count. Large Sources retain their disk path. This stage does not change canonical append code and does not rerun the isolated append matrix.

CPU includes product threads and driver, with 10 ms resolution. Lifecycle includes verification/checkpoint/close/reopen/cleanup; evidence output follows cleanup. Provider/controller are siblings; provider survives product cleanup. Startup/control and complete lifecycle remain in raw phase records. Writes are process-attributed, not physical-device writes. RSS/PSS are sampled maxima, not peaks; allocations unavailable. No median subtraction or multiplied stage ratios.

[Verified compact archive](evidence/compact-navigation-results.tar.xz) and [manifest](evidence/compact-navigation-results.tar.manifest.json) retain all inputs, executable/build identity, commands, phase results and summaries. Original evidence remains available. Correctness: [85 targeted tests](compact-navigation.md). Exact-head FULL and independent review remain required; no merge.
