# Isolated checkpoint admission results

All **120 products pass**: three alternating pairs across both observer modes/all six runtime cases (72), plus warm/cold marginal append at histories 0/64/512/2,048 (48). Runtime checks cover 1,692 full local HTTP/SSE requests and 846 complete semantic request pairs. Model-facing time, native schemas, compiler, normal optimized settings and Rust harness hashes match. The external controller owns sibling provider/product processes through cleanup. No compilation, profiling or compression overlaps the cohort. These are Rust revision measurements; Pi is absent.

Before is source `b87ae908b17dc9b63c2e6993a0c81d10c0960177`, binary SHA-256 `52240c89502584f7ef85cc5ce7b6425ed204c8d70f82d97fca0375a9d0323f45`. After is reporting/build head `016615954f0a154e0c00bbd0d88ec9bbb81c85cf`, production source `0f79f8128d5aea1eca486a7cced822e2a7468adc`, tree `7b0d190bdbd508e946b8b93f0dbe54d1b7e5fd7c`; freshly compiled library/conformance binary SHA-256 `6e61dd2777c7384492fe8c716f41e28faefc3f54159c382fa5753bd3b78e9444`. The build took 9m41s. Test-only whole-checkpoint differential additions at `9101bffa` were not in the measured executable; they change no production behavior.

Values are medians of within-pair after/before ratios. Complete runtime CPU/writes are absolute self counters since exec through cleanup; append lifecycle counters are initialization-to-cleanup deltas including seed. These scopes stay distinct.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | 0.9883 | 0.9910 | 0.9910 | 0.9985 | 0.9860 |
| no-live-subscriber | short-large | 0.9727 | 0.9680 | 0.9821 | 0.9986 | 0.9932 |
| no-live-subscriber | stream | 1.0059 | 1.1538 | 1.0000 | 0.9991 | 0.9778 |
| no-live-subscriber | large | 0.9871 | 1.0000 | 0.8889 | 0.9991 | 0.9801 |
| no-live-subscriber | tool | 0.9460 | 1.0000 | 0.9500 | 0.9993 | 0.9919 |
| no-live-subscriber | cancel | 0.9867 | 1.0000 | 1.0000 | 1.0010 | 0.9758 |
| live-subscriber | short-small | 0.9351 | 0.9346 | 0.9391 | 0.9997 | 0.9887 |
| live-subscriber | short-large | 0.8856 | 0.8922 | 0.9211 | 1.0031 | 1.0004 |
| live-subscriber | stream | 1.0012 | 1.0000 | 1.0000 | 0.9974 | 0.9748 |
| live-subscriber | large | 1.0195 | 1.0000 | 1.0000 | 0.9991 | 0.9847 |
| live-subscriber | tool | 0.9806 | 1.0000 | 1.0000 | 0.9973 | 0.9812 |
| live-subscriber | cancel | 1.0092 | 1.0000 | 1.0000 | 1.0031 | 0.9904 |

Live short-large improves about **11.4% wall and 10.8% CPU**, with 90% paired log intervals [0.8254,0.9602] and [0.8271,0.9615]. Live short-small estimates improve about 6.5%, with intervals [0.8927,1.0033] and [0.8861,1.0097] still crossing 1. No-live short-large improves about 2.7% wall/3.2% CPU; short-small changes are small/uncertain. Writes are essentially unchanged. No-live streaming CPU is adverse at **1.1538**, interval [0.8696,1.3549]; it is retained, not dismissed by tick resolution. Live burst-large wall is also adverse at 1.0195. No universal speedup or overall retention decision follows.

| Mode | History | Append wall | Append CPU | Lifecycle CPU | Lifecycle writes | Sampled anon PSS |
|---|---|---:|---:|---:|---:|---:|
| warm-append | 0 | 0.9673 | 0.5000 | 1.0000 | 1.0006 | 0.9976 |
| warm-append | 64 | 1.0004 | 1.0000 | 0.8750 | 0.9998 | 0.9977 |
| warm-append | 512 | 0.9548 | 1.0000 | 1.0222 | 1.0000 | 0.9981 |
| warm-append | 2048 | 1.0169 | 1.0000 | 0.9742 | 1.0000 | 0.9975 |
| cold-append | 0 | 1.0750 | 1.6667 | 1.0000 | 1.0003 | 0.9974 |
| cold-append | 64 | 0.9926 | 0.8571 | 1.0000 | 1.0000 | 0.9976 |
| cold-append | 512 | 1.0861 | 0.7500 | 1.0333 | 1.0000 | 0.9981 |
| cold-append | 2048 | 1.0589 | 0.5455 | 0.9958 | 1.0000 | 0.9982 |

Cold append wall estimates at histories 0/512/2,048 are adverse at 1.0750/1.0861/1.0589, with intervals [0.9941,1.2240]/[0.9741,1.1700]/[0.8948,1.1583]. Standalone append does not directly use complete-checkpoint fitting, so these are sensitivity results, not claimed canonical improvements. Cold per-operation wall/CPU exclude reopen; whole-phase/lifecycle retains seed, reopened verification, close/checkpoint, sampling and cleanup. Cold connection does not mean cold kernel cache. No whole-request O(1), allocation-saving claim or multiplied-stage Pi gap is asserted.

The new direct `RuntimeClientCheckpoint` test compares final eviction decisions, complete mutated DTOs and encoded bytes against forced structural admission at the aggregate bound minus one/exact/plus one, ordinary and wide-u64 cases, plus an oversized authoritative descriptor. Every individual pending header remains within its item budget. The selected preflight/checkpoint suites pass **23 distinct tests**, including that new matrix; their union with prior 389 targeted passes is 390. The initial exact selector used `tests` rather than `budget_tests` and selected zero; the full checkpoint owner suite exercised the test successfully. The first compile used Display formatting for a structured protocol error; comparison now uses exact structured equality. All original launcher/build logs remain retained. All-target/all-feature lint, formatting and whitespace pass. Successful-proof/fallback diagnostics are still pending on an explicitly unshipped instrumented branch; no hit rate is inferred from these ratios.

The initial capacity precheck stopped before any product, with about 1.58 GiB free. The newly compiled root library was retained in verified gzip form and a duplicate target executable released only after its preserved copy matched SHA-256. The complete cohort then passed the unchanged 1.8 GiB precheck. No sample failed or was rerun for capacity. Historical inactive evidence was also retained losslessly to reserve later build space, with published manifest checks where available; every original byte is recoverable.

[All 300 paired metric rows](evidence/checkpoint-admission/readable/paired-costs.csv), [100 intervals](evidence/checkpoint-admission/readable/paired-uncertainty.json), [input hashes](evidence/checkpoint-admission/readable/input-hashes.json) and a standalone raw reader accompany the [591-file verified archive](evidence/checkpoint-admission-results.tar.xz), 1,727,468 bytes, and [manifest](evidence/checkpoint-admission-results.tar.manifest.json). The reader imports no producer summaries and verifies every archive hash, all 120 raw samples, 600 counter values, 300 ratios and 100 intervals. All original pairs, slow cases, controls, commands, phase endpoints, failed precheck/build attempts and executable-retention proofs remain available.

```sh
python3 docs/pi-durable-cost-model/evidence/checkpoint-admission/readable/recompute.py \
  --archive docs/pi-durable-cost-model/evidence/checkpoint-admission-results.tar.xz \
  --readable docs/pi-durable-cost-model/evidence/checkpoint-admission/readable
```

Intervals use df 2, critical 2.91998558 (two-sided 90%, one-sided 95% upper), assuming independent approximately normal log ratios. CPU is product self user+system including driver/verification, in 10 ms ticks. Memory is sampled RSS/PSS, not peak; allocations are unavailable. Process writes are not physical-device writes. Startup/provider/control, Source/DB/WAL/SHM extents and all lifecycle phases remain explicit. Evidence-file output follows cleanup; unrelated medians are not subtracted.

Measured head `016615954` passed [FULL/Merge 39212897561](https://github.com/Caismis/rustX/actions/runs/39212897561). Independent review found no verified correctness blocker in its source delta; the new direct differential closes its requested test gap. Independent arithmetic/raw review of this new 120-product evidence is still separate. The [current aligned Pi gap](aligned-pi-gap-v3.md) measures prior `b87ae908` production; do not multiply it by these ratios. Incremental live-view/lazy-inspection work and controlled future main integration remain ongoing. Do not merge.
