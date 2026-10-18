# Authentication inside the full candidate

This is a dependency-sensitive leave-one-out comparison. Before restores
independent original-capture authentication on top of full candidate `a3c116fe`,
while retaining the RawValue parser repair, lazy previews, atomic final settlement
and bounded cadence. After is the full candidate with authentication simplified.
Neither branch changes the user's selected scope.

Measured before head: `b8ce4cb649b96affcfcfa22d9f1447d976f8a219`.
Before source tree: `be3d8fea6e6f7dfdae6773d0a4626a8dce98783d`.
Before executable SHA-256: `c53b3401a4c160cff654f50ee7036f1e007dd32a01a23551041b167f16aefdaa`.
After head: `a3c116fe91d7689d86c5009c7b563289d3cd34cc`; executable
`e99efcdbcb608b94e2868b5b5c2f48e6d9c4d67f631e18a602ec6b7f0e1a349a`.
The diagnostic uses gate 76 and rejects other layouts; no migration/dual write.

All 36 samples, 846 full ordered requests and 18 paired comparisons passed.
Three interleaved fresh-process/store pairs per case, matched compiler/flags and
fixture identities, same controller-owned sibling HTTP/SSE provider. Formal
runs did not overlap builds, tests, profiling or compression.

| Workload | Turn wall | Turn CPU | Lifecycle wall | Lifecycle CPU | Sampled anon PSS | Process lifecycle writes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| short-small | 0.9331 | 0.9373 | 0.9506 | 0.9692 | 0.9995 | 0.9977 |
| short-large | 0.9594 | 0.9748 | 0.9939 | 1.0000 | 0.9897 | 0.9972 |
| stream | 1.0013 | 1.0000 | 1.0024 | 1.0000 | 1.0076 | 0.9965 |
| large | 0.9551 | 0.8750 | 0.9316 | 0.9565 | 0.9969 | 0.9934 |
| tool | 1.0014 | 1.0000 | 0.9686 | 1.0000 | 1.0071 | 0.9954 |
| cancel | 1.0221 | 1.0000 | 0.9969 | 1.0476 | 0.9936 | 0.9939 |

Ratios are full simplified candidate / authentication-restored variant, medians
of within-pair ratios. Here short-small wall/CPU improve 6.7%/6.3%, and
short-large improve 4.1%/2.5%. This is stronger short-turn CPU evidence than the
isolated integrity increment; it must not be dismissed as zero benefit. Large
CPU is quantized to very few 10 ms ticks. Stream/tool CPU show no improvement,
cancellation wall regresses 2.2%, and memory is mixed. Lifecycle writes improve
only 0.2–0.7%. Three pairs are limited evidence, not confidence intervals.

The separate matched short-small profile has 544 write commits on both sides,
12,824 → 12,790 WAL frames and 52,834,880 → 52,694,800 frame bytes. This is
small write savings, not a physical-device measurement. Endpoint dbstat owners
can miss intermediate page reuse; mixed/unmapped owners remain in the archive.
No overlapping elapsed scopes are interpreted as exclusive CPU percentages.

The tradeoff remains removal of independent original-capture authentication and
origin-proof walks. Coherently rewritten optional Source content is no longer
independently detected; native canonical authority and retained checksum/geometry
checks remain. This is the highest audit tradeoff among the six changes. Consider
restoring it if the independent witness matters; retaining it under the explicitly
authorized Pi-level inspection contract buys a modest short-turn CPU benefit,
not substantial disk savings. The user chooses; no automatic revert occurred.

CPU includes product/driver self threads at 10 ms resolution; provider/controller
CPU is excluded. RSS/PSS are sampled, not peaks; allocations are unavailable.
Process writes are not physical-device writes. Lifecycle includes verification,
checkpoint, close/reopen and cleanup, with evidence output after counters.
Provider start/control and complete controller wall remain in raw records.

## Correctness-patch review follow-up

Independent review P2 found the separately extracted RawValue patch omitted its
generic test helper. The measured diagnostic's production parser repair was present; its library build was non-test (`profile.test=false`), so that missing
`cfg(test)` helper did not affect the production executable or the formal full
request oracles. The original measured identity remains explicit above.

Diagnostic follow-up `8a24bc99a173ca14899c2721094e683c6e6acca1` completes only that test helper. The source
diff/commit remains separate; it is not represented as a newly measured build.
The standalone corrected patch was applied to its documented `b5840bd9`
baseline and compiled as the complete real library test harness: all 6 preview
and 26 read-layout tests passed on `ac9042ea`, retaining gate 73.
[Commands, identity and readable logs](evidence/rawvalue-correctness/manifest.json)
provide the baseline proof. Diagnostic
variants are not merge candidates and have no claim of complete variant FULL CI.

[Archive](evidence/auth-leaveout/measurements.tar.xz),
[manifest](evidence/auth-leaveout/measurements.tar.manifest.json), and
[formal summary](evidence/auth-leaveout/formal-summary.json) preserve every sample,
profile, exact build identity, and original/corrected diagnostic patches. Originals
remain in the workspace. Reproduce the before build with the verified builder,
then use `compare-revisions.py` with before `pi-auth-restored-build/conformance`,
after `pi-partial-build/conformance`, and a new output directory; the archived
`run-cost-stage.py pi-auth-leaveout pi-auth-restored-build pi-partial-build` records
all commands and verifies each profile identity against its formal counterpart.
