# Checkpoint admission diagnostics

Unshipped instrumentation at `ee8567b94dfe7421e55382cd6db135bf591b8c5f` wraps the exact production predicate from `0f79f812` with one relaxed atomic counter per decision. A fresh normal optimized executable records **3,135 proved / 0 deferred** across twelve complete lifecycle fixtures, covering both observer modes and six cases. All **24 control/diagnostic products pass**, with 564 full HTTP/SSE requests and 282 complete same-engine semantic request pairs. These counts describe these fixtures; they do not establish a universal hit rate or isolate steady-state admissions. Instrumentation is archived as a patch and is absent from formal comparison executables and the PR's production code.

| Mode | short-small | short-large | stream | large | tool | cancel |
|---|---:|---:|---:|---:|---:|---:|
| No live subscriber | 409 | 969 | 34 | 19 | 26 | 21 |
| Live subscriber | 464 | 1,081 | 37 | 23 | 29 | 23 |

One control/diagnostic pair per cell exposes instrumentation/code-layout overhead and noise. Across those pairs, diagnostic/control ratios range from 0.8214–1.2839 wall, 0.8000–1.3333 turn CPU, 0.7895–1.1429 complete CPU, 0.9974–1.0195 process writes and 0.9720–1.0257 sampled anonymous PSS. This is not a statistical bound or an isolated atomic cost. No subtraction is made from formal data. Extra evidence output follows product cleanup.

The ignored diagnostic microbenchmark uses a real captured RuntimeClientCheckpoint from the formal short-large fixture, with an additional explicit native full-u64 variant and controlled structural budget boundaries. Each cell has three alternating pairs of 256 admissions: 48 batches / 12,288 predicate calls. Both predicates must agree on acceptance. The scope is monotonic predicate wall time, not CPU, full checkpoint eviction, request time, or whole-history complexity. New-path timings include atomic bookkeeping.

| Checkpoint | Budget | Quiet-repeat median new/structural |
|---|---|---:|
| Ordinary integers | production | 0.1423 |
| Ordinary integers | structural bound − 1 | 1.2034 |
| Ordinary integers | structural bound | 1.1386 |
| Ordinary integers | structural bound + 1 | 1.1927 |
| Full native u64 | production | 1.1941 |
| Full native u64 | structural bound − 1 | 1.1759 |
| Full native u64 | structural bound | 1.1634 |
| Full native u64 | structural bound + 1 | 1.2121 |

The successful conservative proof takes about 14% of the old predicate time in this input. All explicit fallback cases cost about **14–22% more**, so failed proofs are a real penalty. The earlier micro run, retained without replacement, produced a successful-proof ratio 0.1489 and fallback medians 1.1555–1.2202; lightweight prototype text editing overlapped that first run. The repeat ran alone for 6.97 seconds, without compilation, compression, profiling or editing. A separate 4,096-atomic bookkeeping control took 23,275 ns (5.68 ns/op) in the quiet run; it is not subtracted and does not bound combined overhead.

Reproduction:

```sh
python3 docs/pi-durable-cost-model/evidence/checkpoint-admission-diagnostics/recompute.py
```

The [149-file archive](evidence/checkpoint-admission-diagnostics.tar.xz) contains raw products, controller/provider logs, complete comparisons, both micro runs, build identities/logs, input hash, runner and instrumentation patch. The reader verifies every manifest hash and derives counts, micro ratios and control ratios from retained bytes without importing producer summarizers. It checks recorded oracle outcomes and comparison facts; this **does not independently rerun the semantic oracle**. Parent review has not independently verified this diagnostic archive. Formal CPU remains process-self/thread counters at recorded 100 Hz, driver included; RSS/PSS are sampled, allocations unavailable, writes process-attributed rather than physical-device writes. The [isolated formal checkpoint comparison](checkpoint-admission-results.md) remains the evidence for product costs.
