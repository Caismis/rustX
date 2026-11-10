# Cost and contract decision matrix

The full candidate remains intact. The user selects retained scope; these are
recommendations, not automatic reversions or merge authorization. Production is
`a3c116fe91d7689d86c5009c7b563289d3cd34cc`, source tree
`be0a92467809fd493aad64e8b1dc6b5e0d7a9f61`, on #484 at `7d659c06`.
Later commits correct tests and evidence without changing that production tree.

All formal comparisons use three interleaved pairs per workload, fresh processes
and stores, the same local HTTP/SSE provider, and identical frozen build settings.
Ratios are medians of within-pair ratios. CPU includes product/driver self threads,
with 10 ms ticks. RSS/PSS are sampled maxima, not peaks; allocations are unavailable.
Lifecycle includes verification, checkpoint, close/reopen and cleanup. Evidence
output follows product counters. Process writes are not physical-device writes.
Provider start/control, verification overhead and lifecycle phase records remain
explicit in the linked reports. No isolated ratios are multiplied.

## Recommended scope for user decision

| Change | Contract, dependency and measured cost | Recommendation |
| --- | --- | --- |
| Template metrics (`989d74d1`) | No intended semantic loss; bounded derived widths. Isolated short-small wall/CPU 1.011/1.016, writes 0.999; mixed other cases. Final marginal effect unmeasured. | Optional; no consistent end-to-end gain justifies it on speed alone. |
| Resident bytes (`e1823321`, `c34de111`) | Bounded 16 KiB allowance per Source; larger carriers spill, navigation remains 4 KiB. Matched profile byte files 442→40→0 and byte-carrier reads 40,753→27,356→0; navigation files remain. `/tmp` is tmpfs. General memory/CPU savings unproven. Final marginal effect unmeasured. | Retain for fewer files/connections/reads, accepting the bounded resident allowance. |
| Scoped indexes (`d054be08`) | No intended event/recovery loss; retain null-owner adoption indexes. Isolated short-small writes 0.826 with CPU 1.007; 20,005→16,535 WAL frames, same 672 commits. Reversal must align schema/readers/gate. Final marginal effect unmeasured. | Retain for demonstrated write reduction, subject to final FULL query contracts. |
| Lazy previews (`b5840bd9`) | Moves optional preview work/errors to first access; eager navigation and historical bytes remain. Writes improve only 0.4–1.2%, mixed CPU/memory; first-read indexed SQL/byte cost rises. Shared later inspection changes complicate inverse patch. Final marginal effect unmeasured. | Low-yield complexity candidate to reconsider; no general speedup claim. |
| Authentication removal (`5e37b924`) | Material loss of independent original-capture authentication: coherent optional Source rewrites can pass. Final-context short-small CPU 0.937, wall 0.933; writes only 0.998. RawValue correctness repair must survive any reversal. | Highest semantic rollback candidate, but the final-context ~6.3% short CPU saving is real measured evidence. User must weigh that modest saving against audit loss. |
| Atomic final settlement (`ce8f7270`) | Removes independent ordinary final P/U/C crash cuts; acceptance/events/staging remain atomic and publication follows commit. Tool unknown/replay and Workflow/rejection paths remain distinct. Final-minus-atomic short-small CPU 0.931, writes 0.799; short-large CPU 0.935, writes 0.813. | Retain if user accepts the documented lost crash cuts; meaningful savings persist with cadence held fixed. |
| Bounded cadence (`a3c116fe`) | 100 ms oldest-deadline trigger, 64 KiB/128-frame pressure; synchronous commit-before-live. Delivery includes persistence/scheduling and is not a 100 ms SLA. Larger uncommitted tail and less frequent display; canonical-only tool projection repair is included. | Retain for burst/stream workloads if display/crash-tail tradeoff is accepted. Large wall/CPU 0.270/0.318, writes 0.336; paced-stream writes 0.911. Short-large wall/CPU regress 2.6%/1.7%, tool CPU 9.1%, cancellation wall 3.6%. |

The standalone RawValue whitespace/duplicate-key repair is a correctness fix,
not a discretionary optimization. Preserve it under every scope choice.
The resident bound is a separate increment of the resident carrier; cadence also
contains its necessary canonical tool projection repair. These dependencies must
be resolved explicitly if any subset is selected. No migrations or fallback layouts.

## Direct combined and final-context evidence

[Combined comparison](combined-results.md), [authentication leave-out](authentication-leave-out.md),
[atomic leave-out](atomic-leave-out.md), and [cadence](partial-cadence.md) provide
full commands, executable identities, lifecycle phases, retained samples and manifests.
The authentication restoration diff, build identity and test logs are
[readable here](evidence/auth-leaveout/readable/README.md).

| Workload | Full/#484 wall | CPU | Lifecycle writes | Sampled anon PSS |
| --- | ---: | ---: | ---: | ---: |
| short-small | 0.90952 | 0.93204 | 0.65388 | 0.99364 |
| short-large | 0.92916 | 0.96508 | 0.65160 | 1.00013 |
| paced stream | 0.99662 | 0.85000 | 0.77694 | 0.98997 |
| burst large | 0.27905 | 0.36364 | 0.31287 | 0.98281 |
| tool | 0.87759 | 0.85714 | 0.77322 | 1.00170 |
| cancel | 0.93512 | 1.00000 | 0.92113 | 0.99829 |

Direct verified-main short-small wall/CPU/writes are 0.81032/0.87539/0.62749.
The actual Pi Durable SQLite production adapter is pinned at `90d863f7`:
short-small Rust/Pi medians are 59.998/4.680 ms wall per turn,
91.875/9.375 ms CPU and 80.801/8.875 MiB lifecycle writes. These include the
specified verification/driver scope, not pure engine or database costs. Native
schemas remain explicit (Rust 7 tools/Pi 4). Old desktop/in-process ratios are
not equivalent controls and are not used here.

Marginal warm append versus #484 **regresses** at histories 0/64/512/2048:
wall ratios 1.05045/1.03930/1.05846/1.02383, writes approximately unchanged;
2048 CPU is 1.25 at coarse resolution. Cold-connection wall is
1.04121/1.07244/1.06233/0.89696; cold does not mean cold kernel cache.
Against original main, warm 2048 wall/CPU/writes are 0.61821/0.625/0.63968.
No whole-request O(1) claim follows from the append optimization.

Final-minus-authentication compares the full candidate to restored authentication
with cadence and atomic settlement retained: short-small CPU improves ~6.3%,
short-large ~2.5%, other CPU quantized/mixed; cancellation wall regresses 2.2%.
Profile commits are 544 on both sides, frames 12,824→12,790. Final-minus-atomic
retains cadence and profiles 670→543 commits, 16,342→12,804 frames. The 127-commit
difference is reported as observed, not rounded to 128. Large wall regresses 1.1%
and cancellation writes 0.1% in that comparison. All adverse samples remain.

Final-minus-cadence is its isolated last-stage comparison. Corrected large-output
profile: 145→17 commits, 2302→405 frames. Initial A/A profiles are withdrawn but
preserved; the formal matrix used verified distinct binaries and is unaffected.
Other changes have isolated evidence only; interaction-dependent final marginal
benefits remain unknown. Three pairs are limited evidence, not confidence bounds.
Final exact-HEAD FULL/Merge and independent re-review remain required.

## Completed isolated workload comparisons

These are individual stage comparisons against their preceding measured build,
not marginal effects inside the eventual full combination. Each stage has 36
accepted samples, 846 full ordered requests and 18 paired comparisons.

### Template metrics

Commit [`989d74d1`](https://github.com/Caismis/rustX/commit/989d74d12d211a38eee5ad1f07ec18c0251a0de5); [full report](template-metrics.md).

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 1.0112 | 1.0161 | 1.0185 | 1.0209 | 0.9883 | 0.9994 |
| short-large | 1.0164 | 1.0156 | 1.0020 | 1.0042 | 0.9986 | 1.0000 |
| stream | 0.9812 | 0.9048 | 0.9859 | 0.9143 | 0.9955 | 0.9993 |
| large | 1.0049 | 1.0476 | 1.0002 | 1.0000 | 1.0013 | 0.9994 |
| tool | 0.9919 | 1.0000 | 1.0121 | 1.0000 | 1.0105 | 0.9995 |
| cancel | 0.9452 | 1.0000 | 0.9973 | 1.0000 | 0.9995 | 1.0009 |

### Resident bytes 4 KiB

Commit [`e1823321`](https://github.com/Caismis/rustX/commit/e1823321fa0dce1e3969f571e4eb61bb85aa2924); [full report](resident-source.md).

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 1.0197 | 1.0338 | 0.9975 | 1.0129 | 1.0106 | 1.0008 |
| short-large | 1.0031 | 1.0063 | 1.0021 | 1.0051 | 0.9978 | 1.0003 |
| stream | 1.0008 | 1.0526 | 1.0059 | 1.0303 | 0.9952 | 1.0000 |
| large | 1.0277 | 0.9565 | 1.0193 | 1.0000 | 0.9963 | 0.9997 |
| tool | 0.9802 | 1.0000 | 1.0081 | 1.0000 | 1.0035 | 0.9995 |
| cancel | 0.9975 | 1.0000 | 0.9820 | 0.9524 | 0.9883 | 0.9991 |

### Resident bytes 16 KiB

Commit [`c34de111`](https://github.com/Caismis/rustX/commit/c34de111f5da991e5b92db40a9391d60842b8c49); [full report](resident-bound.md).

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9641 | 0.9553 | 0.9718 | 0.9647 | 0.9954 | 0.9987 |
| short-large | 0.9776 | 0.9814 | 0.9981 | 0.9964 | 0.9965 | 1.0005 |
| stream | 1.0011 | 0.9545 | 1.0051 | 1.0286 | 1.0008 | 1.0000 |
| large | 0.9286 | 0.9545 | 0.9738 | 1.0000 | 0.9976 | 0.9997 |
| tool | 0.9856 | 0.9167 | 1.0360 | 0.9655 | 0.9912 | 0.9990 |
| cancel | 0.8762 | 0.8333 | 0.9780 | 0.9524 | 0.9912 | 0.9991 |

### Scoped event indexes

Commit [`d054be08`](https://github.com/Caismis/rustX/commit/d054be08981dcc98c53c5ba83641b387f26442d4); [full report](event-indexes.md).

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 1.0093 | 1.0068 | 1.0038 | 1.0022 | 1.0052 | 0.8259 |
| short-large | 0.9847 | 0.9940 | 0.9736 | 0.9740 | 0.9985 | 0.8115 |
| stream | 0.9954 | 0.9130 | 0.9984 | 0.9730 | 0.9971 | 0.9266 |
| large | 1.0016 | 1.0000 | 1.0094 | 1.0270 | 0.9893 | 0.9650 |
| tool | 0.9828 | 1.0000 | 1.0166 | 1.0690 | 1.0028 | 0.8713 |
| cancel | 0.9343 | 0.8333 | 0.9775 | 1.0000 | 1.0010 | 0.9305 |

### Lazy state previews

Commit [`b5840bd9`](https://github.com/Caismis/rustX/commit/b5840bd903d2508e693e1a4f80c3dc6173c9e29b); [full report](lazy-state-previews.md).

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9842 | 0.9932 | 0.9898 | 1.0000 | 1.0173 | 0.9876 |
| short-large | 0.9963 | 0.9909 | 0.9791 | 0.9749 | 1.0135 | 0.9894 |
| stream | 0.9964 | 1.0435 | 0.9998 | 1.0270 | 0.9909 | 0.9941 |
| large | 1.0022 | 1.0455 | 0.9826 | 1.0000 | 1.0081 | 0.9960 |
| tool | 0.9451 | 0.8571 | 0.9886 | 1.0000 | 0.9921 | 0.9894 |
| cancel | 0.9789 | 0.8571 | 0.9933 | 1.0000 | 1.0090 | 0.9950 |

## WAL and temporary-resource evidence

| Change | Separate diagnostic | Limits |
| --- | --- | --- |
| Template metrics | No isolated WAL/temp profile in this increment. | Do not infer zero effect from an unavailable measurement. |
| Resident bytes 4 KiB | Matched 40 requests: byte files 442→40, opens 1649→240, pread calls 40,753→27,356, written byte-carrier bytes 692,618→482,467. | Instrumented scope; navigation indexes remain 40 files / 160 opens / 1,966,080 written bytes. `/tmp` is tmpfs. |
| Resident bytes 16 KiB | Matched 40 requests: byte files 40→0, opens 240→0, pread calls 27,356→0, written byte-carrier bytes 482,467→0. | Navigation index work remains; these are not physical-device writes or end-to-end CPU percentages. |
| Scoped event indexes | 32 steady turns: 20,005→16,535 WAL frames; 672 write commits and 256 new events on both sides. Endpoint event-index frames 7824→4337. | Endpoint page owners do not exclude intermediate page reuse; mixed/unmapped pages retained. |
| Lazy state previews | 16,548→16,379 WAL frames; state-field frames 1167→989; 3008 new state fields / 256 events on both sides. | Write commits differ 672→671, so total savings are not attributed purely to preview layout. First reads add indexed metadata/byte work; bounds are in the report, not an assertion of zero cold-read cost. |

## Source integrity isolated comparison

Commit `5e37b924`; [complete report](inspection-integrity.md). This increment also
contains the lexical correctness fix. Three pairs per case; no general gain.

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9737 | 1.0204 | 0.9743 | 1.0151 | 1.0096 | 0.9977 |
| short-large | 0.9444 | 0.9547 | 0.9718 | 0.9723 | 0.9982 | 0.9973 |
| stream | 0.9996 | 0.9048 | 0.9915 | 0.9211 | 1.0031 | 0.9955 |
| large | 0.9260 | 1.0909 | 0.9613 | 1.0270 | 1.0062 | 0.9981 |
| tool | 0.9793 | 1.0000 | 0.9743 | 1.0000 | 1.0193 | 0.9941 |
| cancel | 1.0044 | 1.0000 | 0.9899 | 1.0000 | 1.0007 | 0.9949 |

Separate diagnostic: 672 write commits each; 16,355 → 16,348 WAL frames.
The original-byte authentication loss is material relative to these small savings.
Final-context evidence is summarized above and linked in full.

## Isolated atomic-settlement measurement

Three interleaved paired runs in each of six cases passed: 36 samples, 846 full
ordered requests and 18 paired request comparisons. Predecessor is integrity
`5e37b9240290723483ac00855cee7eee58760c81`; measured atomic production is
`ce8f7270ef65ef279f9c5d6c73135b420074dab0`, source tree
`c13b161837efdbfa98e76afac23a50366274e315`. The clean release builder verified
identical compiler/settings and all benchmark fixture hashes. Executable SHA-256:
`80cddef13e44afc99e0b1178fc180f4849abc8aae98503a166e1de678190ccac`.

All values below are medians of three same-pair after/before ratios. Formal
samples ran after compilation ended and before profiling; none were removed.

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9203 | 0.9286 | 0.9624 | 0.9647 | 0.9979 | 0.7992 |
| short-large | 0.9603 | 0.9529 | 0.9966 | 0.9875 | 1.0080 | 0.8125 |
| stream | 0.9978 | 0.9545 | 1.0004 | 1.0000 | 1.0019 | 0.9281 |
| large | 1.0099 | 0.9545 | 1.0047 | 1.0000 | 1.0045 | 0.9712 |
| tool | 0.9924 | 0.8462 | 0.9904 | 1.0000 | 1.0010 | 0.9056 |
| cancel | 1.0472 | 1.0000 | 0.9917 | 1.0000 | 1.0167 | 0.9980 |

Short-small turn wall/CPU improve about 8.0%/7.1%, with lifecycle writes down
20.1%; short-large writes fall 18.7%. Cancellation turn wall regresses 4.7%,
large wall regresses 1.0%, and memory is mixed. These three-pair observations
are not confidence bounds or proof of a general speedup. The user must weigh
the lost independent final crash cuts against these measured savings.

Separate matched short-small WAL profile: 672 → 544 write commits over 32
steady turns (four fewer per turn), 16,349 → 12,783 frames and 67,357,880 →
52,665,960 frame bytes. Both sides add exactly 256 events, 64 ledger rows,
704 content segments and 3008 state fields. This supports reduced transaction
write amplification, not removal of those native facts. Endpoint dbstat owners
can miss intermediate page reuse; mixed/unmapped owners are retained. No
overlapping elapsed scope is reported as an exclusive CPU percentage.

CPU includes product plus driver, self threads only at 10 ms resolution; the
provider is a controller-owned sibling and stays alive through product cleanup.
Memory is sampled RSS/PSS, not a peak; allocations are unavailable. Process
writes are not physical-device writes. Lifecycle includes verification,
checkpoint, close/reopen and cleanup; evidence-file output follows counters.
Provider startup/control and full controller lifetime remain in raw records.
The comparison is against the immediate predecessor, not Pi and not a
multiplied estimate of the final combination.


## Isolated cadence measurement

Atomic predecessor `ce8f7270ef65ef279f9c5d6c73135b420074dab0` versus cadence
`a3c116fe91d7689d86c5009c7b563289d3cd34cc`, source tree
`be0a92467809fd493aad64e8b1dc6b5e0d7a9f61`. The preserved cadence executable
SHA-256 is `e99efcdbcb608b94e2868b5b5c2f48e6d9c4d67f631e18a602ec6b7f0e1a349a`.
Clean release build identities verify the same compiler, flags and fixture hashes.
The later test/document fixes do not change production or this preserved binary.

All 36 fresh samples, 846 ordered requests and 18 paired comparisons passed.
Medians below are three same-pair ratios, not ratios of unrelated medians.

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9926 | 1.0075 | 1.0006 | 1.0187 | 1.0105 | 0.9980 |
| short-large | 1.0261 | 1.0171 | 1.0174 | 1.0156 | 0.9942 | 1.0007 |
| stream | 0.9971 | 0.9091 | 1.0025 | 0.9730 | 0.9967 | 0.9114 |
| large | 0.2696 | 0.3182 | 0.6300 | 0.5946 | 0.9869 | 0.3362 |
| tool | 0.9708 | 1.0909 | 0.9674 | 0.9615 | 1.0046 | 0.9869 |
| cancel | 1.0362 | 1.0000 | 1.0036 | 1.0000 | 1.0009 | 0.9980 |

Large-output wall/CPU improve about 73.0%/68.2%, with lifecycle writes down
66.4%. Paced-stream writes fall 8.9%; its wall remains mostly provider pacing.
Short turns establish no general gain: short-large wall/CPU regress 2.6%/1.7%,
tool CPU regresses 9.1%, and cancellation wall regresses 3.6%. Short-small
anonymous PSS rises about 1.0%. Three pairs are limited evidence, not confidence
intervals, and all adverse samples are retained.

The corrected large-output diagnostic records 145 → 17 WAL write commits,
2,302 → 405 frames and 9,484,240 → 1,668,600 frame bytes for one measured turn.
This corroborates fewer partial persistence transactions. The short-small
diagnostic has 542 → 543 write commits, 12,794 → 12,780 frames, and
52,711,280 → 52,653,600 frame bytes over 32 turns. That tiny difference is not
attributed solely to layout or cadence, especially with differing commit counts.
Endpoint dbstat ownership does not exclude intermediate page reuse.

Two initial diagnostic profiles accidentally selected the cadence executable on
both sides through chained path substitution. They are explicitly withdrawn:
`pi-partial-profile/WITHDRAWN.json` and
`pi-partial-large-profile/WITHDRAWN.json`. Their original files remain archived.
The formal timing matrix used distinct identity-verified executables throughout
and is unaffected. Corrected profiles use explicit before/after paths and a
distinct-binary guard; both identities were additionally checked against the
formal build SHAs. The stage controller now verifies each profile side against
its formal counterpart, with the withdrawn A/A case as a negative check.

CPU is product/driver self CPU at 10 ms resolution; provider and controller are
excluded. RSS/PSS are sampled maxima, not peaks; allocations are unavailable.
Writes are process-attributed, not device writes. Lifecycle includes checkpoint,
verification, close/reopen and cleanup; evidence output follows counter capture.
Provider start/control and complete controller wall remain explicit in raw
records. Profiles ran separately from timings; no build/test/profile/compression
overlap occurred during formal samples. Source temp files live on tmpfs; neither
WAL frame bytes nor process writes imply physical-device savings.

The stage comparison is also final-minus-cadence: it includes the necessary
canonical-only tool projection repair. Its benefit is workload-dependent and
comes with less frequent live publication and a larger uncommitted crash tail.
It remains synchronous commit-before-live with pressure exceptions; it is not
identical to Pi's timer-only persistence behavior.
