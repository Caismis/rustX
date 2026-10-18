# Atomic final settlement with cadence held fixed

Before is diagnostic `13e09b13c2a8b19455cdbbf6a23b693a066126ad`, which restores separate ordinary
P/U/C final commits while retaining bounded 100 ms cadence, canonical tool
projection and every Source change. After is full candidate
`a3c116fe91d7689d86c5009c7b563289d3cd34cc`. Before uses gate 77, rejects other
layouts and is not a migration or a selected revert.

Before source tree: `774a0a1d81e7ed51fad9e76b2618b445e842226a`.
Before executable SHA-256: `f94da7f9fdf8f013ba1b03e4d3f50ef03bc458a6c8d642006ec823a16a66b644`.
After executable: `e99efcdbcb608b94e2868b5b5c2f48e6d9c4d67f631e18a602ec6b7f0e1a349a`.
The verified optimized build records match compiler, flags and fixture hashes.

All 36 fresh samples, 846 full ordered requests and 18 paired comparisons pass.
Each case has three interleaved pairs. Formal runs did not overlap builds, tests,
profiles or compression; every sample remains available. Ratios below are
medians of within-pair full/separate-final ratios.

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9257 | 0.9310 | 0.9573 | 0.9518 | 1.0007 | 0.7987 |
| short-large | 0.9576 | 0.9353 | 0.9728 | 0.9675 | 1.0032 | 0.8134 |
| stream | 0.9901 | 1.0000 | 0.9904 | 1.0000 | 1.0003 | 0.9226 |
| large | 1.0107 | 0.8333 | 1.0218 | 1.0000 | 0.9952 | 0.9159 |
| tool | 0.9874 | 0.8333 | 0.9781 | 1.0000 | 0.9922 | 0.9051 |
| cancel | 0.9947 | 1.0000 | 1.0180 | 1.0500 | 1.0003 | 1.0010 |

With cadence fixed, short-small wall/CPU improve about 7.4%/6.9% and lifecycle
writes fall 20.1%; short-large CPU improves 6.5% and writes fall 18.7%. Large
wall regresses 1.1%, cancellation writes are slightly adverse, and memory is
mixed. Large/tool CPU ratios span few 10 ms ticks; do not overstate precision.

Separate short-small profile: 670 → 543 WAL write commits, 16,342 → 12,804
frames and 67,329,040 → 52,752,480 frame bytes over 32 steady turns. The 127
commit difference is not the exact 128 of the isolated profile, so retain the
scheduling variation rather than rounding it into a stronger claim. Endpoint
dbstat owners may miss intermediate page reuse. These are WAL write commits,
not all SQL transactions or physical-device writes.

This supports retaining atomic settlement under the explicitly authorized
Pi-level finalization contract: the write reduction survives the cadence change.
Its independent final crash cuts are still lost; a provider completion in an
aborted final transaction may be absent after recovery. Accepted native final
result/terminal state remains atomic, with observed I/O errors and separate
external-tool unknown/replay semantics. The user selects retained scope.

CPU includes product/driver self threads at 10 ms resolution. The provider is a
controller-owned sibling and remains alive through product cleanup. RSS/PSS are
sampled, not peaks; allocations are unavailable. Lifecycle includes verification,
checkpoint, close/reopen and cleanup; evidence output follows counter capture.
Provider start/control and complete controller wall remain explicit. This is a
diagnostic performance variant, not a complete variant FULL or merge claim.

[Archive](evidence/atomic-leaveout/measurements.tar.xz),
[manifest](evidence/atomic-leaveout/measurements.tar.manifest.json), and
[formal summary](evidence/atomic-leaveout/formal-summary.json) retain all samples,
profile data, exact identities and the reproducible diagnostic patch. The
archived `run-cost-stage.py pi-atomic-leaveout pi-separate-final-build
pi-partial-build` records the commands and checks profile identities against the
formal builds. Original artifacts remain in the cloud workspace.
