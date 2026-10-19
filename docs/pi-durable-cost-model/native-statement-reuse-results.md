# Isolated native metadata statement results

Before source: `b6c5b4a67775c4db36540e5ee840afcce9ccc45b`; executable SHA-256 `2786906735965b5685ab5ccd2f71f3f86a36d335efc79a8661b9368b2607493b`. After source: `b87ae908b17dc9b63c2e6993a0c81d10c0960177`, production tree `f0943a155a83249499c335e3352b2a9ccba7ed72`; executable SHA-256 `52240c89502584f7ef85cc5ce7b6425ed204c8d70f82d97fca0375a9d0323f45`. Reporting head `a7f2aec8` adds evidence only. Both freshly compiled the root library and conformance executable with identical normal optimized settings, compiler and Rust harness. The reviewed workload-3 clock and strengthened oracles are identical. This is a Rust revision comparison; Pi is absent.

All **120 samples in the complete cohort pass**: three interleaved pairs in both observer modes/all six runtime cases (72), plus warm/cold marginal append at histories 0, 64, 512 and 2,048 (48). Runtime checks cover 1,692 full local-provider requests and 846 paired complete semantic inputs, preserving model-facing time exactly. Native tool schemas match within this Rust comparison. No builds, profiles or compression overlap this complete cohort. Provider and product are controller-owned siblings through cleanup.

Values below are medians of within-pair after/before ratios. Complete CPU/writes are absolute self counters through cleanup for runtime; append lifecycle counters are initialization-to-cleanup deltas including seed. These scopes are deliberately distinct. No stage ratios are multiplied.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live-subscriber | cancel | 1.0070 | 0.8000 | 1.1333 | 1.0010 | 1.0002 |
| no-live-subscriber | large | 1.0126 | 1.0000 | 0.9412 | 1.0000 | 1.0005 |
| no-live-subscriber | short-large | 0.9597 | 0.9531 | 0.8691 | 0.9994 | 0.9979 |
| no-live-subscriber | short-small | 1.0444 | 1.0348 | 0.9668 | 0.9975 | 1.0090 |
| no-live-subscriber | stream | 1.0084 | 1.0000 | 1.0385 | 1.0000 | 1.0034 |
| no-live-subscriber | tool | 1.1574 | 1.2500 | 1.0500 | 1.0000 | 1.0050 |
| live-subscriber | cancel | 0.9434 | 1.0000 | 0.9444 | 0.9990 | 0.9939 |
| live-subscriber | large | 0.9971 | 1.0000 | 1.0000 | 0.9991 | 0.9828 |
| live-subscriber | short-large | 1.0024 | 1.0085 | 0.9132 | 0.9995 | 0.9981 |
| live-subscriber | short-small | 1.0213 | 1.0000 | 0.9620 | 0.9998 | 1.0008 |
| live-subscriber | stream | 1.0139 | 1.0714 | 1.0800 | 1.0000 | 1.0124 |
| live-subscriber | tool | 0.9418 | 0.9000 | 0.9565 | 0.9980 | 0.9960 |

The candidate does **not establish a consistent steady-turn benefit**. No-live short-large improves about 4% wall/5% CPU, while short-small and tool are adverse. Live short-turn CPU is essentially unchanged. Complete CPU decreases in the short cases, which include extensive later historical verification; that saving must not be described as pure engine or database cost. Writes and sampled memory are essentially unchanged.

Live stream CPU has a paired median of 1.0714 and 90% log-ratio interval **[1.0180, 1.1813]**; this adverse result remains visible despite 10 ms CPU resolution. Its wall interval is [0.9963, 1.0282]. No-live tool wall is 1.1574, with a wide interval [0.9032, 1.5337]; it is an adverse estimate rather than a proven universal regression. The separate, earlier observer-demand live-cancellation regression remains open: this SQL comparison uses the repaired demand implementation on both sides and cannot resolve that earlier delta.

| Append mode | History | Append wall | Append CPU | Lifecycle CPU | Lifecycle writes | Sampled anon PSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| warm | 0 | 0.9691 | 1.5000 | 1.0000 | 0.9994 | 1.0024 |
| warm | 64 | 0.9808 | 1.0000 | 1.0000 | 1.0000 | 1.0008 |
| warm | 512 | 1.0005 | 1.3333 | 0.9583 | 1.0000 | 1.0006 |
| warm | 2048 | 0.9912 | 1.0000 | 0.9733 | 1.0000 | 0.9988 |
| cold | 0 | 0.8805 | 0.8000 | 1.0000 | 0.9997 | 1.0000 |
| cold | 64 | 0.9833 | indeterminate | 1.0000 | 0.9998 | 1.0008 |
| cold | 512 | 0.9554 | 1.2500 | 1.0167 | 1.0000 | 1.0006 |
| cold | 2048 | 1.1098 | 0.8750 | 0.9758 | 1.0000 | 0.9970 |

Cold append wall at history 2,048 is adverse at 1.1098, interval [0.8410, 1.3056]. Warm wall changes are small with intervals crossing 1. Cold per-operation brackets and wall exclude reopen; whole-phase costs retain reopen/checkpoint/sampling, and lifecycle includes seed, close, reopened verification and cleanup. CPU brackets include their sampling overhead. One cold-64 CPU observation is quantized to zero, so its complete interval and median are indeterminate; it is not dropped. No whole-request O(1) claim follows from these marginal appends.

## Capacity failure and sensitivity

The first attempt ran out of workspace capacity after **62 accepted samples**: all 36 no-live samples and 26 live samples. Ten later live products failed during temporary-directory creation with `StorageFull`/OS error 28 before provider handshake. Append directories could not be created. All original files, failures and earlier accepted/slow samples remain retained. The normal sandbox launcher then also failed to create `/workspace/.git`; a one-time automatically approved recovery command removed only regenerable compiler archives. Further compiler metadata cleanup brought total released cache to about 1.96 GB. All three preserved executable hashes were rechecked unchanged; no source, measurement, assertion, timeout, provider or workload changed. The complete cohort began after a 1.8 GiB capacity precheck.

The earlier accepted cohort is shown separately in the readable inputs, with no pooling or selective removal. Its no-live short-small/short-large/tool wall medians are 1.0216/1.0280/1.0014; live short-small is 0.9475. Other live groups have only two pairs and receive no interval or selected-subset median. This sensitivity reinforces the uncertainty rather than selecting whichever cohort is favorable.

## Profiles and reproducibility

Sixteen separate control/profile diagnostics cover both builds, both modes and both short cases. CPU sampling uses gperftools at 250 Hz, including all product threads and harness work. Each stack is assigned once in its disjoint partition; ownership and operation totals are not overlapping elapsed-scope percentages. Preparation signals before/after: no-live short-small **74/75**, short-large **76/62**; live **117/124** and **133/123**. These do not demonstrate a consistent preparation reduction. Newly cached owners lose some preparation signals while other native owners gain them; contention for the unchanged 16-entry cache is a hypothesis, not a proven cause. Complete checkpoint fitting remains substantial: live **50/58** and **56/63** signals, against total **506/544** and **577/593**.

Single control/profile pairs show diagnostic overhead/noise: turn wall ratios range 0.9626–1.1807, CPU 0.9921–1.2034, sampled anonymous PSS 1.0386–1.0825. No overhead median is subtracted. Initial diagnostic failures are retained: a 115-byte Unix-socket pathname exceeded its 108-byte structure, then short control/CPU labels collided before a CPU product started. Unique short paths and preflight checks corrected only the diagnostic launcher; complete formal samples are unaffected.

[Readable paired costs](evidence/native-statements/readable/paired-costs.csv), [all uncertainty/indeterminate groups](evidence/native-statements/readable/paired-uncertainty.json), [input hashes](evidence/native-statements/readable/input-hashes.json) and [independent recomputation](evidence/native-statements/readable/recompute.py) expose both cohorts. Independent parsing verifies **182 accepted raw samples, 910 metric inputs, 455 paired rows and 134 positive three-pair intervals**; 26 groups remain incomplete or quantized. The paired log-t method uses df 2 and critical 2.91998558 (two-sided 90%, equivalent one-sided 95% upper), assuming independent approximately normal log ratios; this small cloud study is exploratory.

The [verified 1,314-file, 4,861,032-byte raw archive](evidence/native-statement-reuse-results.tar.xz) and [manifest](evidence/native-statement-reuse-results.tar.manifest.json) include commands, build identities, all counter/phase endpoints, full ordered oracles, failures, recovery proof, profiles and overhead controls. The standalone recomputation also checks every archive member's hash. Original cloud files and optimized binaries remain intact. CPU is self user+system ticks with 10 ms resolution; RSS/PSS are sampled, not peaks; allocations are unavailable. Process-attributed writes are not physical-device writes. DB/WAL/SHM and Source file extents remain explicit in the raw records. Verification and evidence-file output are separate; file serialization follows cleanup. Startup/provider/controller costs remain visible.

Actual PR base remains `7d659c06f692277d4711c54b579f9fe0f8d8792d`; latest verified main is `3c355f152d405a201482d53b6e45497df4f44a7f`, with ancestry checked. Formal runs began only after reviewed oracle `ce3c3c6a` passed [FULL/Merge 39200833903](https://github.com/Caismis/rustX/actions/runs/39200833903), tested merge `f1854c816c1638a1e2ab0b5d61925b707fc9212c`. SQL checkpoint `a7f2aec8` subsequently passed [FULL/Merge 39203612782](https://github.com/Caismis/rustX/actions/runs/39203612782), tested merge `ffb8d497e63a123af457f9a4692cc61136983a48`, with scoped-clean parent source review. Arithmetic/archive review of these results remains separate. These measurements do not justify broad retention as an established performance win or Pi parity. Incremental live-view/deep-inspection work and strict cancellation diagnosis remain ongoing. Do not merge.
