# Direct combined cloud comparison

Measured production is `a3c116fe91d7689d86c5009c7b563289d3cd34cc`, source tree
`be0a92467809fd493aad64e8b1dc6b5e0d7a9f61`. Later fixture/document commits leave
that production tree unchanged. The PR body records the final review head and CI.
Actual dependency/base is #484 at `7d659c06f692277d4711c54b579f9fe0f8d8792d`
(measured production `7ed05395`, same native source tree). Verified main is
`3c355f152d405a201482d53b6e45497df4f44a7f`, containing merged #483/#480.

This round has 204 passing formal samples: 108 runtime/engine samples across
three matrices, plus 96 warm/cold append samples across two baselines. Each
workload/history has three interleaved pairs with fresh processes and stores.
The runtime matrices validate 2,538 full ordered requests in total. No failure
or slow sample is removed, and no isolated stage ratios are multiplied.

All Rust executables use the same verified optimized build, compiler, settings
and complete recorded fixture hashes. Preserved binary SHA-256 values:

- Main: `d6623b03b0326d4548d58af01d89a133c3640a0320a9aa0acbc4e385c1f30f18`.
- #484: `78e044bee701759c17884e2b3813c8f10990ee4e92996008b6a8e65ef243c223`.
- Full candidate: `e99efcdbcb608b94e2868b5b5c2f48e6d9c4d67f631e18a602ec6b7f0e1a349a`.

CPU includes product and driver, all self threads at 10 ms resolution; it excludes
the sibling provider and external controller. Memory is sampled RSS/PSS, not peak
or allocation counts. Process-attributed writes are not physical-device writes.
Formal runs did not overlap compilation, tests, profiling or compression.

## Full six-change candidate versus #484

Median of same-pair after/before ratios; lower is lower measured cost.

| Case | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9095 | 0.9320 | 0.9332 | 0.9370 | 0.9936 | 0.6539 |
| short-large | 0.9292 | 0.9651 | 0.9408 | 0.9432 | 1.0001 | 0.6516 |
| stream | 0.9966 | 0.8500 | 1.0070 | 0.9444 | 0.9900 | 0.7769 |
| large | 0.2791 | 0.3636 | 0.6928 | 0.5946 | 0.9828 | 0.3129 |
| tool | 0.8776 | 0.8571 | 0.9552 | 1.0000 | 1.0017 | 0.7732 |
| cancel | 0.9351 | 1.0000 | 0.9859 | 0.9545 | 0.9983 | 0.9211 |

## Full candidate versus actual main, including #481 foundation

Median of same-pair after/before ratios; lower is lower measured cost.

| Case | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.8103 | 0.8754 | 0.8758 | 0.9043 | 0.9963 | 0.6275 |
| short-large | 0.7646 | 0.8169 | 0.8690 | 0.8933 | 1.0018 | 0.6198 |
| stream | 0.9909 | 0.8000 | 0.9874 | 0.8889 | 0.9896 | 0.7091 |
| large | 0.2459 | 0.3043 | 0.6291 | 0.5789 | 0.9747 | 0.3007 |
| tool | 0.7667 | 0.7857 | 0.9574 | 0.9667 | 0.9967 | 0.6773 |
| cancel | 0.8124 | 0.8333 | 0.9799 | 0.9545 | 0.9929 | 0.8155 |

The six additions reduce short-small wall/CPU about 9.0%/6.8% and lifecycle
writes 34.6% against #484; short-large writes fall 34.8%. Large-output wall
falls 72.1%. Memory remains mostly flat; no general memory reduction is established.
These are three-pair observations, not confidence bounds. Against main, which
also includes the original #481 Source/append work, short-small wall/CPU improve
19.0%/12.5% and writes 37.3%; short-large wall/CPU improve 23.5%/18.3%.

The separate combined short-small WAL profile records 672 → 544 write commits,
20,009 → 12,801 WAL frames and 82,437,080 → 52,740,120 frame bytes over 32
steady turns. Endpoint page ownership can miss intermediate page reuse. These
are write-commit/frame counts, not all SQL transactions or physical-device I/O.

## Marginal canonical append, separate from context construction

Cold means a fresh store connection before each measured append, not a flushed
kernel page cache. Warm measures the append phase after seeding. Cold operation
counters bracket individual appends, including counter-sampling overhead but
excluding reopen; whole-phase costs with reopen remain in the raw summaries.
Small CPU windows are quantized; an undefined ratio is shown as a dash.

| Baseline | Mode | History | Append wall | Append CPU | Marginal writes | Lifecycle writes |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| #484 | warm | 0 | 1.0504 | 0.6667 | 0.9985 | 0.9969 |
| #484 | warm | 64 | 1.0393 | 1.0000 | 1.0000 | 0.9988 |
| #484 | warm | 512 | 1.0585 | 1.0000 | 1.0000 | 0.9999 |
| #484 | warm | 2048 | 1.0238 | 1.2500 | 1.0000 | 1.0000 |
| #484 | cold | 0 | 1.0412 | 1.4000 | 1.0000 | 0.9983 |
| #484 | cold | 64 | 1.0724 | 1.0000 | 1.0000 | 0.9993 |
| #484 | cold | 512 | 1.0623 | 1.2500 | 1.0000 | 0.9999 |
| #484 | cold | 2048 | 0.8970 | 0.8333 | 1.0000 | 1.0000 |
| main | warm | 0 | 0.9944 | 1.0000 | 0.9985 | 0.9969 |
| main | warm | 64 | 0.9687 | 0.7500 | 0.9971 | 0.9981 |
| main | warm | 512 | 0.8504 | 1.0000 | 0.8836 | 0.9222 |
| main | warm | 2048 | 0.6182 | 0.6250 | 0.6397 | 0.7852 |
| main | cold | 0 | 1.0610 | — | 1.0000 | 0.9987 |
| main | cold | 64 | 0.9496 | 1.0000 | 0.9971 | 0.9978 |
| main | cold | 512 | 1.0384 | 1.0000 | 0.8791 | 0.9208 |
| main | cold | 2048 | 0.8660 | 0.6000 | 0.6808 | 0.7845 |

The six additions do not establish a general marginal-append improvement over
#484: writes are essentially unchanged and all four warm wall medians regress
2.4–5.8%. Cold small-history wall also regresses; cold 2048 improves in this run.
Keep these regressions visible rather than using runtime gains to erase them.
The original #481 foundation retains its large-history benefit versus main:
2048 warm append wall/CPU ratios are 0.618/0.625 and marginal writes 0.640.
Whole-request context construction is still not O(1).

## Actual Pi Durable with the same local HTTP/SSE provider

Pi is pinned to `90d863f7c1183b965f099855715d6e88c7b8a52c`, Node 24.19.0,
with unchanged tracked source. This is actual `packages/durable` SQLite through
production `openAICompletionsApi()`, not coding-agent JSONL or an in-process fake.
Both engines use the same local provider scripts, model/protocol parameters,
ordered history, chunk scheduling, tool and cancellation scenarios. Their native
tool schemas remain explicitly different (rustX 7 / Pi 4); complete native
expected-input oracles and paired core request equality checks both pass.

The following are absolute medians from this matrix, not ratios of unrelated
medians and not pure engine/DB costs. No desktop measurement or old 49–57x
in-process-provider ratio is used.

| Case | rustX turn ms | Pi turn ms | rustX CPU ms/turn | Pi CPU ms/turn | rustX lifecycle writes MiB | Pi lifecycle writes MiB | rustX sampled anon MiB | Pi sampled anon MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 59.998 | 4.680 | 91.875 | 9.375 | 80.801 | 8.875 | 146.238 | 110.082 |
| short-large | 61.757 | 4.593 | 94.688 | 7.812 | 205.152 | 20.777 | 234.180 | 162.953 |
| stream | 1562.973 | 1643.066 | 170.000 | 490.000 | 4.418 | 0.844 | 101.023 | 105.344 |
| large | 48.433 | 114.487 | 70.000 | 150.000 | 4.121 | 0.672 | 100.844 | 77.816 |
| tool | 82.802 | 133.563 | 110.000 | 160.000 | 5.887 | 0.938 | 101.094 | 91.062 |
| cancel | 43.237 | 116.898 | 60.000 | 150.000 | 3.832 | 0.621 | 100.133 | 89.148 |

Pi remains much faster on repeated short turns. RustX is faster in this run's
large-output, tool and cancellation turn scopes, but still writes more. Native
contracts, schema, driver verification and startup costs differ; neither overall
equivalence nor an unavoidable-contract explanation of the remaining gap is
claimed. Allocation counts and isolated engine-only CPU/memory are unavailable.

## Startup, lifecycle, measurement overhead and storage

Each product/provider pair belongs to an external controller. The provider stays
alive through product cleanup and is reaped only by that controller, so its child
I/O cannot enter product counters. Full lifecycle records retain initialization,
verification, shutdown, checkpoint, writer close, reopen verification and cleanup.
Evidence serialization/file output occurs after the final counter endpoint;
controller complete wall includes that output and provider teardown.

Sampling and driver work remain in the measured product scopes. Their standalone
CPU overhead is not independently calibrated or subtracted. Verification phases
are separately attributed within each process, never by subtracting unrelated
medians. Provider CPU is excluded/unmeasured; necessary startup/control costs are
in explicit wall fields and lifecycle phases. The following short-small startup
and complete-lifecycle medians expose costs outside the steady-turn table.

| Metric | rustX | Pi |
| --- | ---: | ---: |
| Product startup to ready (ms) | 4.026 | 368.987 |
| Provider startup (ms) | 69.626 | 70.726 |
| Provider teardown (ms) | 25.068 | 18.704 |
| Complete controller lifetime (ms) | 3683.924 | 999.145 |
| Self CPU before first sample (ms) | 0.000 | 550.000 |
| Self CPU process start through cleanup (ms) | 4540.000 | 1230.000 |
| Writes process start through cleanup (MiB) | 80.801 | 8.875 |

DB/WAL/SHM logical and allocated sizes at before-close, checkpoint and closed
endpoints are retained in raw samples and lifecycle details. Source temp counts
and bytes are separate diagnostic evidence; `/tmp` is tmpfs. Rust lists files
recursively while Pi's native root listing also contains a workspace directory,
so their unqualified other/total footprint fields are not equivalent engine
comparisons. No sampled endpoint is called a peak and no logical/allocated size
is called physical-device traffic.

## Reproduction and remaining review gates

[Raw archive](evidence/combined-measurements/measurements.tar.xz) and
[manifest](evidence/combined-measurements/measurements.tar.manifest.json) include
all 204 samples, full ordered oracles, build/hardware identities, the actual
controller commands and separate combined profile. Original evidence and all
preserved binaries remain in the cloud workspace. The archived
`run-final-cost-comparisons.py` runs the comparisons sequentially and fails on any
rejected sample. `run-cost-stage.py` verifies compiler/settings/fixtures and
profile identities. All outputs require new directories, retaining failed runs.

The dependency-sensitive authentication and atomic-settlement leave-outs are
reported separately in the decision matrix. The user chooses retained scope;
there is no automatic revert or merge. Final exact-head FULL/Merge and independent
review remain required. A fixture-only FULL failure history is preserved in
[evidence/final-validation](evidence/final-validation).
