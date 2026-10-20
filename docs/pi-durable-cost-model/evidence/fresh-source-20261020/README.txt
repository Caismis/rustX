PR #485 fresh-machine Native Source evidence — preliminary work results for independent review

Repository: Caismis/rustX. No main/shared PR #485/old branch update or merge.
Measured baseline: 623b41b869a95c888eb04f907cbf607232699a2f
Geometry: 89d6b618eeba8afaf4b95af4406057330baa60a6
Scoped Tools facts: 31668f7a74b63446c538e00ed41ebfc42f0bc1a8
89 and 316 are separate branches, not sequential stages. No multiplied stage ratio or old-machine speed ratio is used. No Pi product was run in this cohort.

144 products, 4 cohorts: each candidate versus a fresh same-machine baseline, no-live-subscriber/live-subscriber, all 6 fixed cases, 3 alternating pairs. Provider processes were siblings; fixed model-facing time is 2026-10-19 00:00:00. No real model API. The original controller/compare scripts and workloads are unchanged. An external wrapper restored matching compiled helpers BEFORE controller.run; those copies are excluded from product startup counters and retained separately.

All 144 product/controller producer validations succeeded. 3,384 captured requests generated 1,692 FULL semantic Rust/Rust request comparisons, including the seven native tool definitions. The provided compare-http.mjs uses sameEngine=true and asserts the entire normalized request, not Rust/Pi core-only equality. The exact time text is preserved; declared text representation/JSON argument spelling and exact fixture path normalization remain. Literal HTTP-byte equality is not asserted. Comparisons are baseline-versus-89 and baseline-versus-316, not a separate 89-versus-316 cohort.

All three root libraries and conformance targets were actually compiled at opt-level=3/debuginfo=0; Cargo fresh:false means rebuilt during this invocation. Root features are empty, debug assertions/overflow checks false, no profiling feature/preload. Exact build/profile/executable/library/helper/host/driver identities are in build-and-driver-identities.json and original build logs. The 562 external artifact graph records match exactly across all variants. Required binaries/libraries remain preserved in the measurement workspace; the archive contains their identities rather than large executable bytes.

Readable data: native-readable-v4/paired-costs.csv has each pair numerator and denominator for 50 metrics (3,600 rows); paired-summary.csv has 1,200 summaries and paired ranges/intervals. All lifecycle phase costs, complete counters since exec, initialization-to-cleanup deltas, and external-controller wall windows remain separate. Complete self CPU includes driver/test-runtime work. Writes are process-attributed, not physical device traffic. Memory is maximum observed RSS/PSS/anonymous PSS, not a true peak or allocation count. The scoped cache is 1 MiB PER ENTRY; old and pending entries may coexist. Total real memory is not asserted to be 1 MiB.

Ratios are within-pair candidate/baseline, never ratios of unrelated medians. A zero/unavailable baseline has no ratio. Intervals are two-sided 90% log-t with n=3, df=2 and critical 2.919985580355516, assuming approximately normal independent log ratios; their center is the geometric mean, not the median. All slow/adverse runs remain. A small cohort is not universal performance acceptance.

Preliminary counterexample: geometry live short-large median paired wall ratio 1.482617 and CPU ratio 1.409605; respective 90% log-t intervals [1.315157,1.625547] and [1.301939,1.517676]. Geometry has favorable no-live cases but does not show universal improvement. Scoped no-live short-large wall/CPU ratios are 0.917959/0.910959, while live short turn intervals are broad and do not establish benefit. Direct actual-Pi remeasurement is deferred; no Pi-gap reduction claim follows.

Native arithmetic/interval verification reproduced 6,048 values against the published summaries; 144 recorded controller intervals do not overlap. This is independent counter arithmetic, not a separately reimplemented semantic request oracle. Original producer oracle results remain disclosed producer evidence. The initial standalone reader incorrectly expected measurement_version=3; aligned samples are measurement_version=4/workload_version=3, as the unchanged controller requires. Only the reader assertion was corrected; no product was rerun or raw sample changed. The correction record is retained.

Archive paths products/native-baseline-vs-* contain every raw sample/process/comparison/log/provider output. Sibling cohort manifests, helper restoration logs and monotonic product interval logs retain start/end/order evidence. native-matrix-orchestration.json lists actual cohort commands and UTC boundaries. The archive MANIFEST.json gives every member's bytes/SHA-256; the producer packer verified all archived bytes against original files. Original files remain intact.

Independent arithmetic rerun after extraction (Python standard library only):
python3 native-delivery/read-native-pairs.py --build-root . --output recomputed products/native-baseline-vs-geometry-no-live-subscriber products/native-baseline-vs-geometry-live-subscriber products/native-baseline-vs-scoped-no-live-subscriber products/native-baseline-vs-scoped-live-subscriber

No source fix or merger is proposed by these measurements. Browser work is separate and still pending. Its unchanged fixture uses a fixed performance_eligible:false label; later results require a separate conditional cohort manifest and distinct visible-row/wire-drain metrics, with the previously disclosed rendering limitations.

Public CSV copies use LF newlines for repository whitespace checks. Every parsed cell equals the archived original CSV, whose CRLF bytes remain preserved in the raw archive.

Browser follow-up:see browser/README.txt for all18scoped products and the stopped geometry cohort,raw-data delivery,visible-row vs logical wire-drain,counter limits and geometry attribution plan. Native CSV complete_self_cpu_ms_since_exec and complete_process_write_mib_since_exec are cumulative through after_cleanup; later evidence-JSON emission and final OS-exit CPU/rusage were not captured. Numeric Native results are unchanged.

Minimal attribution follow-up: see minimal-attribution/README.md for four serial control/CPU products on preserved baseline/geometry binaries, actual wait4 process CPU, signal sampling limits, full raw profiles and the separate fixture-only diagnostic proposal SHA. No causal fix or performance acceptance follows; fixture browser execution awaits independent exact-SHA review.

Diagnostic follow-up: see diagnostic-followup/README.md. One reviewed geometry browser attempt stopped at the 5GiB guard during Docker Created preparation, before browser start; its incomplete capture and private-data policy are explicit. Existing libc profile leaves were resolved offline with exact matching official Debian debug symbols; no further product, rebuild or causal optimization claim.

Read-only storage/ownership review: see disk-ownership-review/README.md. Matching Moby28.4.0 shows two VFS rootfs copies during creation; capacity budget and hardlink-aware task-owned occupancy are explicit. Existing binaries/frontend can be reused without compile. No files/cache/image deleted, no Sourceedit or browserrerun; recommend adding3GiBusablepersistentheadroom ratherthan discarding compileddependencies.
