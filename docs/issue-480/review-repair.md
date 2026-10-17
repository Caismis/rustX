# Review repair: complete inputs and lifecycle accounting

This report supersedes the request-oracle and ordinary CPU/I/O acceptance claims in [the historical AFTER report](after.md). The earlier raw runs, including failures and slow samples, remain unchanged. Its process wall windows and separate syscall trace already included deferred checkpoint/close work; review found a gap in the ordinary CPU/I/O endpoints, not missing wall or trace accounting.

## Correctness and accounting changes

Every ordered wire request is now compared with a complete independent expected body: the entire user/assistant history, initial time context, exact model/options, and the seven pinned native tool schemas. The Read case includes the exact call and tool result. The expectation comes from the fixed workload and checked-in schema fixture, not the implementation's current reconstructed history. Only the independently checked UTC time and temporary workspace path are normalized for expected-value comparison; wire/frozen equivalence retains their actual values. JSON tool argument spelling is compared semantically.

For each request the harness checks its ordered native start event, provisional/frozen identity, model and invocation, Original message references, semantic fingerprint, and an independent projection of the reconstructed frozen request against the entire wire body. It repeats those checks after cold read-only reopen and compares all retained plain request/snapshot values. Five negative tests reject omitted or reordered older history even when wire and frozen inputs agree, mismatched frozen content/time, changed tool schema, and mismatched request identity/Original reference.

The runner requires the exact benchmark test to report one pass, a valid version-2 sample, and all expected live/reopened checks. Exit zero with no matching test, missing JSON, malformed JSON or incomplete oracle output fails the run and is retained. Five runner tests and three accounting tests pass.

Ten process endpoints span before initialization, initialization, turns, live verification, shutdown/relay join, driver drop, final writer close, reopened verification, cleanup and provider teardown. Adjacent deltas and the complete first-to-last window report process user+system CPU and every available `/proc/self/io` counter. The separate measured-turn CPU/I/O metric remains explicitly narrower. Whole-lifecycle accounting includes seed turns, instrumentation and full verification; it excludes OS/test-runtime startup, final sample serialization/output and the provider child's CPU/I/O. Provider teardown wait is included in wall time. Missing platform counters are null, never zero. CPU uses 10 ms kernel tick resolution on this host, so small individual phases can report zero ticks.

## CI fixture repairs and retained failure

The pre-review FULL run [39061954770](https://github.com/Caismis/rustX/actions/runs/39061954770) failed macOS full and therefore Merge verification. Four new WAL fixture failures supplied noncanonical `/var` temporary paths to a root owner using canonical `/private/var` paths. The fixtures now derive their owned paths from the canonical root; production containment checks are unchanged. All ten focused WAL tests pass locally.

The fifth failure was reproduced locally with diagnostics: while terminal background publication was deliberately parked, the separate runtime root attempt adopted its pending notification, changing the durable read mutation revision from 0 to 1 beneath the old published cut. The existing test admission gate now isolates that competing transition during the publication assertion. No assertion, timeout or production behavior was relaxed. The repaired test passed once and ten consecutive isolated repetitions. The standalone initial follow-up command accidentally used an unqualified exact filter and ran zero tests; it is not counted. The fully qualified failing reproduction and all eleven repaired runs each executed exactly one test. Both the failing diagnostic and successful logs are retained. Final-head FULL CI, including macOS, remains required; these local Linux checks do not substitute for it.

The repair adds no Source optimization and calls no real model APIs. [Build provenance](evidence/review-repair/build-provenance.json) verifies the exact same harness/oracle/schema overlay on Stage A and AFTER. The new measurements use ordinary default-feature release binaries, fresh process/store per sample, warm OS cache, and no concurrent task-owned build/test. SQLite 3.46.0 DELETE/FULL versus 3.53.2 WAL/NORMAL remains a dependency-plus-storage comparison, not an isolated PRAGMA experiment.

## Revised paired results

The retained revised matrix has 36 accepted executions and 0 failed executions. See [raw manifest and samples](evidence/review-repair/paired/manifest.json) and [complete phase/counter summary](evidence/review-repair/paired-summary.json). All attempts and slow values are retained. Ratios use only complete matching pairs; individual summaries include all accepted observations. These few samples do not estimate reliability rates.

Median of per-run measured-turn medians, with full range in brackets. Process wall includes startup and result output; the separately bounded lifecycle window below does not.

| Case | BEFORE turn ms [range] | AFTER turn ms [range] | Paired A/B | Process wall s B → A |
|---|---:|---:|---:|---:|
| short-small | 129.62 [107.57–130.49] | 77.47 [69.13–78.35] | 0.604 | 6.522 → 4.139 |
| short-large | 133.26 [126.19–138.74] | 78.77 [68.43–83.68] | 0.568 | 38.128 → 11.959 |
| stream | 1666.99 [1654.23–2713.70] | 1621.75 [1593.38–1672.06] | 0.956 | 1.964 → 1.935 |
| large | 621.13 [578.40–6398.25] | 191.50 [190.93–211.20] | 0.308 | 0.924 → 0.483 |
| tool | 237.30 [219.90–307.72] | 111.25 [111.25–116.65] | 0.469 | 0.521 → 0.399 |
| cancel | 108.94 [89.51–131.82] | 54.68 [49.78–62.16] | 0.457 | 0.393 → 0.351 |

CPU is process user+system, including harness work and excluding the provider child. Values below are medians of per-run deltas. Linux `write_bytes` is process-attributed storage accounting, not physical media traffic or a count of SQLite calls. Whole includes initialization, seed/measured turns and all subsequent phases. Post-turn includes verification and teardown.

| Case | Measured CPU ms/turn B → A | Whole CPU ms B → A | Post-turn CPU ms B → A | Whole write MiB B → A | Post-turn write MiB B → A |
|---|---:|---:|---:|---:|---:|
| short-small | 127.50 → 110.00 | 6200.00 → 5210.00 | 1070.00 → 820.00 | 209.54 → 149.61 | 0.00 → 0.36 |
| short-large | 136.25 → 112.81 | 18390.00 → 14480.00 | 5560.00 → 4150.00 | 525.30 → 391.48 | 0.00 → 1.46 |
| stream | 270.00 → 250.00 | 460.00 → 440.00 | 40.00 → 30.00 | 7.81 → 6.50 | 0.00 → 1.18 |
| large | 350.00 → 240.00 | 530.00 → 410.00 | 30.00 → 30.00 | 19.96 → 14.06 | 0.00 → 0.67 |
| tool | 180.00 → 130.00 | 360.00 → 310.00 | 40.00 → 30.00 | 10.68 → 9.30 | 0.00 → 0.46 |
| cancel | 70.00 → 70.00 | 260.00 → 240.00 | 30.00 → 20.00 | 4.61 → 4.91 | 0.00 → 1.14 |

WAL moves measurable writes into final writer close and reopen; cancellation whole-lifecycle writes increase from 4.61 to 4.91 MiB. The two short-turn workloads improve both measured-turn latency and whole-lifecycle CPU, but this does not make WAL free of deferred work.

Phase detail for the two short-turn workloads is below; the linked summary retains all nine phase deltas, seven I/O counters, whole and post-turn totals for every run in all six cases. Medians of phases need not sum to the median whole.

| Case / phase | CPU ms B → A | Write MiB B → A | Wall ms B → A |
|---|---:|---:|---:|
| short-small / initialization | 130.00 → 140.00 | 0.86 → 1.60 | 233.41 → 218.76 |
| short-small / turns | 4980.00 → 4260.00 | 208.67 → 147.66 | 5202.25 → 3077.20 |
| short-small / verification | 530.00 → 400.00 | 0.00 → 0.00 | 517.90 → 387.40 |
| short-small / shutdown | 10.00 → 10.00 | 0.00 → 0.00 | 9.09 → 8.34 |
| short-small / driver_drop | 0.00 → 0.00 | 0.00 → 0.00 | 1.65 → 2.08 |
| short-small / final_writer_close | 0.00 → 10.00 | 0.00 → 0.33 | 5.38 → 10.74 |
| short-small / reopen_verification | 510.00 → 390.00 | 0.00 → 0.03 | 511.51 → 397.30 |
| short-small / cleanup | 10.00 → 0.00 | 0.00 → 0.00 | 3.15 → 3.54 |
| short-small / provider_teardown | 10.00 → 10.00 | 0.00 → 0.00 | 23.46 → 24.08 |
| short-large / initialization | 140.00 → 140.00 | 0.86 → 1.60 | 233.38 → 235.29 |
| short-large / turns | 12690.00 → 10180.00 | 524.44 → 388.42 | 32288.20 → 7506.26 |
| short-large / verification | 2760.00 → 2000.00 | 0.00 → 0.00 | 2757.28 → 2003.01 |
| short-large / shutdown | 10.00 → 20.00 | 0.00 → 0.00 | 13.91 → 17.09 |
| short-large / driver_drop | 0.00 → 0.00 | 0.00 → 0.00 | 3.39 → 3.26 |
| short-large / final_writer_close | 10.00 → 10.00 | 0.00 → 1.43 | 6.43 → 13.84 |
| short-large / reopen_verification | 2740.00 → 2090.00 | 0.00 → 0.03 | 2738.89 → 2095.19 |
| short-large / cleanup | 0.00 → 10.00 | 0.00 → 0.00 | 5.57 → 5.54 |
| short-large / provider_teardown | 30.00 → 30.00 | 0.00 → 0.00 | 49.63 → 52.37 |

## Costs and retained limitations

| Case | First-consumed publication ms B → A | Sampled max RSS MiB B → A | Max live logical MiB B → A | Final writer open/close ms B → A |
|---|---:|---:|---:|---:|
| short-small | 99.58 → 63.89 | 176.61 → 178.62 | 9.21 → 13.83 | 4.48 → 9.00 |
| short-large | 104.16 → 65.11 | 200.31 → 199.47 | 19.74 → 24.15 | 4.94 → 12.16 |
| stream | 1619.66 → 1610.16 | 154.71 → 156.32 | 1.39 → 4.22 | 8.16 → 13.91 |
| large | 552.63 → 49.51 | 155.40 → 156.18 | 1.62 → 5.49 | 4.74 → 7.42 |
| tool | 105.52 → 53.83 | 157.09 → 156.72 | 1.91 → 5.84 | 4.35 → 6.31 |
| cancel | 98.73 → 51.74 | 155.07 → 154.84 | 1.33 → 2.73 | 4.17 → 9.70 |

First-consumed publication is a consumer metric, not first emission; stream/cancel drains follow explicit provider gates. RSS is sampled, not a high-water mark. Final writer open/close includes validation and checkpoint/close; it is not an exclusive checkpoint timer. Every accepted sample asserts zero remaining WAL frames before cleanup. Retained raw samples contain live and closed file inventories.

- short-small: slowest accepted process 6.607 s (before, pair 2), maximum measured turn within it 157.66 ms.
- short-large: slowest accepted process 38.883 s (before, pair 0), maximum measured turn within it 158.55 ms. Seed turns are also retained: turn 40 takes 3893.88 ms in that run; pair 1 has a 2712.95 ms seed turn. These delays remain in whole-lifecycle and process totals.
- stream: slowest accepted process 3.786 s (before, pair 2), maximum measured turn within it 2713.70 ms.
- large: slowest accepted process 7.290 s (before, pair 2), maximum measured turn within it 6398.25 ms.
- tool: slowest accepted process 0.642 s (before, pair 2), maximum measured turn within it 307.72 ms.
- cancel: slowest accepted process 0.420 s (before, pair 0), maximum measured turn within it 131.82 ms.

The historical syscall trace, Pi runs and native Source profiles remain in [the original report](after.md); they were not regenerated with this full-input oracle and do not replace the revised acceptance matrix. Their trace/deferred-cost accounting, unchanged Source work, pinned Pi comparison and unavailable perf/allocation measurements remain explicitly qualified there. No Source optimization was added.

## Validation status

The five negative request-oracle tests, eight Python guard/accounting tests, ten focused WAL tests, eleven isolated repaired publication-test executions, formatting, whitespace and Clippy passed locally. The earlier broad local suite is retained in [validation](validation.md). The previous FULL run failed as recorded above. The next final-head FULL run and independent re-review must pass before the coordinator considers merge.

