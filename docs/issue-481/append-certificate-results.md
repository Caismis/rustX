# Incremental append certificate measurements

Production candidate `563279ac8b6aeac29b36e75f5682195df9dd6c0b` is compared with
Source-metadata stage `76ecc9d765e5c63080950e7dc5710a32b0b1caa7`, using the same
corrected benchmark fixture. This isolates the connection-local certificate;
compact page/spool changes are not included. All 120 fresh-process samples pass:
36 runtime, 24 warm append, 24 cold append and 36 same-provider Pi samples.
Each case/history has three paired, alternating runs. No build or profiler ran
during these formal matrices. Failures from development checks remain archived.

Ratios are median same-pair candidate/baseline ratios, not ratios of independent
medians. CPU is product plus driver at 10 ms resolution, memory is sampled rather
than peak, and writes are process-attributed rather than physical-device writes.
Warm CPU covers the entire append phase; cold CPU sums operation windows with
counter/sampling overhead. Native allocator counts are unavailable. Evidence JSON
is written after cleanup counters. Startup, provider control, seed, close,
checkpoint, reopen verification and cleanup remain separately recorded.

## Marginal append

| Mode | Seed history | Append wall ratio | Append CPU ratio | Lifecycle write ratio | Sampled anonymous PSS ratio |
|---|---:|---:|---:|---:|---:|
| warm | 0 | 0.9830 | 1.0000 | 1.0000 | 1.0065 |
| warm | 64 | 1.0063 | 1.0000 | 1.0000 | 1.0054 |
| warm | 512 | 0.9408 | 1.0000 | 1.0000 | 1.0052 |
| warm | 2048 | 0.9147 | 0.6667 | 1.0000 | 1.0044 |
| cold | 0 | 0.9990 | 2.0000 | 1.0000 | 1.0034 |
| cold | 64 | 0.9856 | 2.5000 | 1.0000 | 1.0056 |
| cold | 512 | 0.9887 | 1.0000 | 1.0000 | 1.0045 |
| cold | 2048 | 1.1033 | 1.5000 | 1.0000 | 1.0024 |

Warm marginal wall improves 8.5% at 2,048 retained messages and 5.9% at 512;
the 64-message control is slightly adverse. Cold append at 2,048 is 10.3% slower.
The certificate does not remove cold validation and adds version checks. Coarse
CPU windows are noisy, especially at small histories; their large adverse ratios
are retained rather than called pure engine CPU. Lifecycle gains include the
warm seeding phase even in cold-mode samples and do not negate cold-operation
regressions. No disk or sampled-memory improvement is established.

Deterministic work-counter tests prove that warm native appends reuse validated
counts, while first/cold/invalidation paths use the existing validator. These are
logical validation counts, not whole-request complexity claims. Native frontier,
publication, duplicate/order and checkpoint checks remain. The accepted arbitrary
out-of-band historical-corruption detection tradeoff and full consumer/mutation
audit are in [append validation](append-validation.md).

## Whole runtime

| Case | Turn wall ratio | Turn CPU ratio | Lifecycle write ratio | Sampled anonymous PSS ratio |
|---|---:|---:|---:|---:|
| short-small | 0.9671 | 0.9597 | 1.0001 | 1.0013 |
| short-large | 1.0073 | 1.0092 | 0.9996 | 0.9991 |
| large | 0.9971 | 1.0476 | 0.9997 | 1.0118 |
| stream | 0.9954 | 1.0000 | 0.9987 | 0.9985 |
| tool | 0.9665 | 0.9286 | 0.9996 | 0.9864 |
| cancel | 1.0119 | 1.0000 | 1.0000 | 1.0080 |

Short-small wall/CPU improve 3.3%/4.0%; short-large is slightly adverse. This is
not a general whole-request speedup, and context construction remains separate
unavoidable work. The same-provider Pi matrix retains the actual pinned Durable
SQLite production adapter, native Tool-schema differences and full ordered
oracles. It is a contemporaneous engine comparison, not a pure DB-cost comparison.
The runtime revision matrix and Pi matrix each validate 846 complete requests.

## Identity and validation

- Baseline executable SHA-256: `ab315719267cecb3c4f74527349c6f972fd4dbea839dbdb00585b2643f714e5c`.
- Candidate executable SHA-256: `a9c7e712c54340671af0f6387920e259df754ec26659bb398bdafdb433d030d5`.
- Shared fixture SHA-256: `0c9b61828fe19270f32308eb382cc2dbe3f0dbb45abceff3790f1cf98e056f16`.

The 383-test durable suite passed. All-target/all-feature Clippy passed after
semantics-preserving match cleanup. The final 15-test focused rerun includes
sparse frontiers, exact 8,192 admission behavior, new-ID error timing, rollback,
trigger replacement, checkpoint Original deletion, schema changes, overflow
fail-closed behavior, reopen, and an external writer between COMMIT and seating.
Final combined FULL/Merge and independent review are still required.

The [compact evidence archive](evidence/certificate-stage.tar.xz) includes every
sample, process result, oracle output, full logical/allocated DB/WAL/temp footprint,
summary, build identity and development-check failure. Its
[SHA-256 manifest](evidence/certificate-stage.tar.manifest.json) is checked against
the archive; original evidence remains in the cloud workspace.

## Build identity review resolved for this stage

Independent review found that the earlier stage-1 cold baseline was mislabeled:
a shared Cargo target returned the metadata executable for both inputs. That
cold comparison is withdrawn. An independent metadata rebuild at 08:27 UTC on
2026-10-18 first cleaned the root crate, then freshly compiled both its library
and conformance target. Its SHA-256 is byte-identical to this stage's baseline,
`ab315719267cecb3c4f74527349c6f972fd4dbea839dbdb00585b2643f714e5c`.
The certificate candidate also freshly compiled its library and conformance
target. This confirms the distinct production identities for all stage-2 pairs.
The [identity audit](evidence/build-identity-audit.json) retains both the invalid
stage-1 records and clean confirmation. Original evidence is unchanged.
[Plain pair inputs](evidence/pair-inputs) expose executable hashes and every
numeric pair input; independent recomputation matches all published paired medians.
