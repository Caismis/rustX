# Strict-input observer-demand cancellation control

The earlier timestamp-normalized observer-demand series reported a live cancellation wall ratio of 1.158 and CPU ratio of 1.25, with both 90% log intervals above 1. The fresh strict-input control does **not reproduce that point estimate**, but its uncertainty still permits a regression. The earlier evidence remains intact; this result does not declare the regression resolved.

The before control is local diagnostic commit `29ca99808010e8c9e1d5dd551afc059f705dfde2`: the original pre-demand `fbe6ae0c` production source plus the independently reviewed model-facing clock plumbing and current Rust benchmark harness. The archive includes its reconstruction script and tree/source proofs; the control commit is not advertised as a fetchable remote object. Preparation proves the clock files were unchanged between the original control and the oracle parent before overlay. The after executable is the reviewed oracle source `b6c5b4a67775c4db36540e5ee840afcce9ccc45b`, including demand-started observation and the checkpoint-cut repair. Their production delta is confined to Runtime Client host/projection, plus one host test fixture. The later SQL candidate is absent from both. Harness hashes, compiler, optimized settings and model-facing time agree. Both builds freshly compiled the root library and conformance target. Before executable SHA-256: `08d759b07d54ba459ae0ce6de61fd571a2e304c6f42dcfab688089dc0e952baf`; after: `2786906735965b5685ab5ccd2f71f3f86a36d335efc79a8661b9368b2607493b`.

All **12 fresh cancellation runs pass**, three alternating pairs in each observation mode. All six ordered full semantic request comparisons pass with exact model-facing time `2026-10-19 00:00:00`. Each product uses its production HTTP/SSE adapter and a clean process/store. The external controller owns sibling product/provider processes and retains the provider through product cleanup. Cancellation follows the provider's observed partial-output gate; there is no timing sleep, artificial matching delay or real inference service. Twelve additional before-control smokes cover all six cases in both modes; these validate correctness and support no cost claim.

Ratios are after/before, within pair. Intervals use the predeclared paired log-t calculation, three pairs, df=2: two-sided 90% (equivalently one-sided 95% upper bound). They assume independent normally distributed log ratios and are exploratory cloud estimates.

| Mode | Metric | Paired median | 90% interval |
|---|---|---:|---:|
| No live subscriber | Cancellation wall | 0.9695 | [0.8820, 1.0915] |
| No live subscriber | Turn CPU | 1.0000 | [0.5887, 1.2963] |
| No live subscriber | Complete self CPU | 1.0588 | [0.9073, 1.1450] |
| Live subscriber | Cancellation wall | 0.9809 | [0.8608, 1.1350] |
| Live subscriber | Turn CPU | 1.0000 | [0.7880, 1.1238] |
| Live subscriber | Complete self CPU | 0.9444 | [0.8725, 0.9818] |

All five metric families, including complete process writes and sampled anonymous PSS, are retained in the [readable paired values](evidence/cancellation-v3/paired-costs.csv) and [full uncertainty](evidence/cancellation-v3/paired-uncertainty.json). No adverse pair is dropped and no historical/new samples are pooled. Phase records distinguish initialization, cancellation, late snapshot/verification, shutdown, checkpoint/close, reopen and cleanup. Product CPU is self user+system in 10 ms ticks, including driver/verification; small one-turn CPU differences are coarse. Evidence-file output follows cleanup and is excluded from product counters. Provider start/control/teardown is explicit in process records. Memory is maximum sampled RSS/PSS, not peak. Allocations are unavailable. Process-attributed writes are not physical-device writes. No unrelated medians are subtracted.

The [147-file archive](evidence/observer-demand-cancellation-v3.tar.xz) and [manifest](evidence/observer-demand-cancellation-v3.tar.manifest.json) preserve control preparation, fresh optimized build identity/logs, all control smokes, all cancellation inputs/results/counters, commands and comparisons. Original cloud files/executables remain available. This standalone reader checks every archive hash and recomputes all 30 paired metric rows and ten intervals directly from the raw counters:

```sh
python3 docs/pi-durable-cost-model/evidence/cancellation-v3/recompute.py \
  --archive docs/pi-durable-cost-model/evidence/observer-demand-cancellation-v3.tar.xz \
  --readable docs/pi-durable-cost-model/evidence/cancellation-v3
```

Further diagnosis remains open; these data do not establish Pi parity or justify removing the checkpoint-cut correctness repair.
