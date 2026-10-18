# Fresh whole-issue comparison against actual origin/main

Actual base/latest verified main is `3c355f152d405a201482d53b6e45497df4f44a7f`,
an ancestor containing #483/#480. Candidate production is
`7ed053952b34705419e8b4ffffcc24ae3e423b0d`, source tree
`ccc658e1f1186f7ebcf8ef36570a8fc5bd4cbf6e`.
Both sides were independently root-clean rebuilt with Rust 1.98.1, default release
settings and identical complete five-file fixtures. Base executable SHA-256 is
`d6623b03b0326d4548d58af01d89a133c3640a0320a9aa0acbc4e385c1f30f18`;
candidate is `78e044bee701759c17884e2b3813c8f10990ee4e92996008b6a8e65ef243c223`.
This direct fresh matrix is authoritative for the combined change; isolated
stage ratios are not multiplied or extrapolated into it.

All 84 revision samples pass (36 runtime, 24 warm append, 24 cold append).
The separate final same-provider Pi matrix adds 36 passing samples. Combined
with the compact-page isolated matrix, there are 204 final formal samples and
2,538 fully validated requests across the three runtime/engine matrices.

Every case/history has three paired, alternating fresh-process/store runs.
Ratios are medians of same-pair ratios, not ratios of unrelated medians. No slow
samples were removed. Formal runs did not overlap compilation, tests or profiles.
CPU includes product and driver at 10 ms resolution; memory is sampled RSS/PSS,
not peak; writes are process-attributed, not physical-device writes. Evidence
serialization/output follows cleanup counters. Native allocation counts are
unavailable. Whole-request context construction is not O(1).

## Runtime

| Case | Turn wall ratio | Turn CPU ratio | Lifecycle write ratio | Sampled RSS ratio | Sampled anonymous PSS ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 0.8553 | 0.8917 | 0.9651 | 0.9995 | 0.9887 |
| short-large | 0.8931 | 0.9245 | 0.9510 | 1.0030 | 0.9999 |
| large | 0.9503 | 0.9167 | 0.9607 | 1.0020 | 0.9919 |
| stream | 0.9983 | 1.0500 | 0.9130 | 0.9918 | 0.9953 |
| tool | 0.8861 | 0.8667 | 0.8752 | 0.9933 | 0.9967 |
| cancel | 0.9222 | 0.8750 | 0.8861 | 1.0138 | 0.9975 |

Short-small wall/CPU improve 14.5%/10.8%; short-large improves 10.7%/7.5%.
Lifecycle write reductions are 3.5%/4.9%. Streaming CPU is 5.0% adverse and its
paced wall is essentially unchanged. There is no general resident-memory gain;
all sampled RSS/PSS results, including small adverse observations, are retained.

## Native marginal append, separate from context construction

| Mode | History | Append wall ratio | Append CPU ratio | Marginal write ratio | Lifecycle write ratio |
|---|---:|---:|---:|---:|---:|
| warm | 0 | 0.9350 | 1.0000 | 1.0000 | 1.0000 |
| warm | 64 | 0.9863 | 0.6667 | 0.9971 | 0.9986 |
| warm | 512 | 0.8147 | 0.7500 | 0.8836 | 0.9224 |
| warm | 2048 | 0.5953 | 0.6250 | 0.6397 | 0.7853 |
| cold | 0 | 1.0545 | 1.0000 | 1.0000 | 1.0000 |
| cold | 64 | 0.9973 | 1.3333 | 0.9971 | 0.9985 |
| cold | 512 | 0.9627 | 1.0000 | 0.8791 | 0.9210 |
| cold | 2048 | 0.9119 | 0.5000 | 0.6808 | 0.7845 |

At 2,048 retained messages, warm append improves 40.5% wall, 37.5% CPU and 36.0%
marginal writes. Cold append improves 8.8% wall and 31.9% marginal writes at that
size, while empty-history cold append is 5.5% slower. Small CPU windows are
quantized/noisy; no zero denominator is converted into an invented ratio.
Lifecycle gains include seeding and cannot be used to erase cold-operation
regressions. The isolated certificate's 10.3% cold-2,048 regression remains in
[its own report](append-certificate-results.md).

## Same-provider actual Pi Durable comparison

Pi is pinned to `90d863f7c1183b965f099855715d6e88c7b8a52c`, Node 24.19.0,
with unchanged tracked source. Both production adapters call the same local
HTTP/SSE provider using the same core request/history/response/tool/cancellation
script. Native tool schemas (rustX 7 / Pi 4) and retained contracts differ.
No real model API/credential, desktop baseline or old in-process faux-provider
ratio is used. These are runtime-plus-driver measurements, not pure engine/DB costs.

| Case | rustX turn ms | Pi turn ms | rustX CPU ms/turn | Pi CPU ms/turn | rustX lifecycle writes MiB | Pi lifecycle writes MiB |
|---|---:|---:|---:|---:|---:|---:|
| short-small | 65.679 | 4.502 | 96.250 | 9.375 | 123.750 | 8.875 |
| short-large | 71.593 | 4.823 | 104.688 | 8.125 | 314.949 | 20.777 |

Pi remains substantially faster in these repeated short-turn workloads. Native
schema/trace/durability differences are explicit; neither equivalence nor that
all remaining overhead is inherently required by rustX contracts is claimed.

## Startup, complete lifecycle and storage

The [plain lifecycle details](evidence/final-lifecycle-details.json) retain three
samples per case/side, phase deltas, controller/provider startup and teardown,
CPU from process start through cleanup, and logical/allocated DB/WAL/SHM sizes.
Each delta is computed inside one process before aggregation. Self CPU includes
all product threads/driver, excludes child/provider CPU, and is quantized at 10 ms.
Absolute final counters additionally expose loader/import/test-harness CPU before
the first lifecycle sample. Post-cleanup evidence serialization is excluded from
those CPU/write counters but included in controller complete wall.

In the final Pi short-small matrix, product-startup-to-ready medians are 4.16 ms
for rustX and 348.04 ms for Pi; provider startup is 68.80/69.60 ms. Product self
CPU from process start through cleanup is 4,780/1,160 ms, and controller complete
wall is 3,923.00/958.23 ms. Pi has 520 ms of self CPU before its first lifecycle
sample (rustX rounds to zero ticks), so steady-turn values must not conceal startup.
These full-lifecycle quantities include initialization, seeds, verification,
shutdown, checkpoint/close, reopen and cleanup, with their individual phases exposed.

In the direct whole-issue short-small comparison, before-close DB logical bytes
are 9,641,984 → 8,843,264 and WAL logical bytes 4,808,072 → 4,540,272; closed DB
allocated bytes are 9,646,080 → 8,843,264. Short-large closed DB allocated bytes
are 20,426,752 → 20,172,800. These file sizes are distinct from process-attributed
write traffic and cannot establish physical-device writes.

Other/total footprint listings are not an equivalent cross-engine comparison:
rustX lists recursive files, while Pi lists direct root entries including the
workspace directory. DB/WAL/SHM entries are separately identified. Source temporary
files are sampled independently in the diagnostic profile; short-lived files and
other temporary-file classes can be missed. No sampled maximum is called a peak.

## Reproducibility and validation

The cloud host has a four-CPU quota, five affinity CPUs and a 16 GiB cgroup limit.
Source files live on `/tmp` tmpfs; runtime stores live on `/workspace` overlay.
OS caches were not dropped. [Build/environment identities](evidence/final-build-identities.json),
[plain pair inputs](evidence/pair-inputs), [profiles](final-structural-profiles.md),
and the [archive](evidence/final-structural.tar.xz) / [manifest](evidence/final-structural.tar.manifest.json)
retain commands, toolchain, fixtures, successes/failures and all slow samples.
The invalid same-binary cold comparison remains separately marked and preserved
in the earlier identity-repair evidence; it is not included here.

Local validation: 393 durable SQLite tests, 21 focused tests, all-target/all-feature
Clippy with warnings denied, formatting, 37 Python tests and eight Pi oracle tests.
All selected FAST jobs passed on production `7ed05395`; its merge gate correctly
requires FULL for this high-risk change. See [PR #484](https://github.com/Caismis/rustX/pull/484)
for exact evidence-complete head, latest integrated main, final FULL/Merge link and
independent review status. No timeout, interleaving gate, assertion, recovery,
audit or publication contract was weakened.
