# Final combination and bounded selection

Base/main is `3c355f152d405a201482d53b6e45497df4f44a7f`. The final production
combination restores narrow SQL statement reuse in `ef11f5de`. Its Rust source,
tests and Cargo trees exactly match independently reviewed and measured
`99da5d688178227ecb0c75d7db8ac5c73cb3918f`. The preserved optimized executable
SHA-256 is `3757014962aa24e831f8562cfd04d729f4435690d575cba17f9e3a0c2f9cfd55`.
`final-production-identity.json` in the archive records all tree identities.
This avoids presenting a different build as the measured executable.

## SQL decision

The earlier SQL-only stage showed no broad runtime or aggregate SQLite allocation
benefit, so it was provisionally removed in `218b5043`. We measured that candidate
in 204 fresh-process samples: Source-only, further-combined, original-baseline,
direct SQL removal, Pi, and marginal append. Every oracle passed. Its executable
SHA-256 is `2edfdae2a030cf29a07dde1e797a6e973448ec31af7e87d083a072afd82ed293`.
Archive names beginning `selected-` refer to this subsequently rejected candidate.

The direct removal comparison was adverse. Before running three further pairs,
we recorded [selection criteria](sql-retention-criteria.md), timestamp
2026-10-18 06:01:04.589587 UTC and SHA-256
`d9d8f0db33dbfa4838155a94357112247f7d285c1f2ff90a8fda3569e7db425d`.
These criteria were chosen after seeing the first three pairs: this is a practical
confirmation rule, not preregistered statistical inference. The additional 36
samples balance execution order across six pairs per workload. All passed.

Ratios below are **without SQL reuse / with SQL reuse**, medians of matched pair
ratios. Above one favors retaining reuse.

| Workload | Wall | CPU | Sampled RSS |
|---|---:|---:|---:|
| short-small | 1.0414 | 1.0479 | 1.0185 |
| short-large | 1.0271 | 1.0212 | 1.0132 |
| cancel | 1.0527 | 1.0000 | 1.0260 |
| large | 1.0106 | 1.0714 | 1.0179 |
| stream | 0.9968 | 1.0541 | 1.0236 |
| tool | 1.0457 | 1.0417 | 1.0260 |

Equal-weight geometric means of the two repeated-turn workload ratios are
1.03447 CPU and 1.03421 wall. Five of six CPU pairs in each favor reuse. All
predefined CPU, wall, memory/write and singleton guards pass. The result clears
the 3% practical CPU threshold narrowly. We retain the small existing-cache patch
for this combination; this is not evidence of a universal benefit or a proven
interaction mechanism. Earlier contradictory SQL-only findings and every slow
sample remain. No additional selection matrices were run after this decision.
Separate baseline cohorts are not used to rank candidates by unrelated medians.

## Final measured result and limitations

[Final combination matrices](continued-results.md) contain the chosen executable's
192 samples, Source-only, append, original baseline, Pi and cancellation results.
[Profiles](continued-profiles.md) retain complete diagnostic phase counts and
limitations. Whole-issue short-small wall/CPU ratios are 0.9613/0.9906; short-large
are 0.9245/0.9256. At 2048 history entries marginal append wall/CPU/write ratios
are 0.7232/0.7143/0.6397; empty-history wall/CPU are adverse at 1.0277/1.5 with
coarse CPU ticks. The integrity scan remains; this is not whole-request O(1).
Total sampled RSS does not improve. Diagnostic requested allocation bytes fall
about 10%, but allocator replacement is intrusive and native allocation counts
remain unavailable. Stream CPU and some singleton wall results regress.

Removing unused `active_json` rewrites and small Source SQL index construction
removes work. Bounded observation reservations, right-sized buffers and statement
reuse chiefly reduce constant costs. Large cold Source access remains physically
bounded. Original byte carriers, witness verification, canonical authorities,
P<U<C commits and publication/recovery semantics remain intact.

The fair Pi control still shows a large gap: short-small rustX/Pi mean-turn wall
medians are 66.086/4.947 ms, CPU 95/9.375 ms and lifecycle writes 127.883/8.875 MiB.
Short-large wall is 73.017/4.617 ms. These are product-plus-driver measurements,
not pure engine costs. Both production adapters use the same local fake HTTP/SSE
provider and actual pinned Pi Durable SQLite; native tool schemas remain explicit.
We claim neither Pi parity nor that the remaining cost is intrinsically required
by stronger contracts. Further structural research is deferred pending verification.

CPU uses product self ticks at 10 ms resolution. Provider/controller startup and
control are separately retained. Product/provider are sibling processes and the
provider outlives product cleanup. Evidence JSON writes follow product counter
endpoints; verification work and full close/checkpoint/reopen/cleanup stay in the
reported lifecycle. RSS/PSS are sampled, not peaks; writes are process-attributed,
not physical-device writes. Profiles run outside timings. All formal matrices use
clean processes/stores and unchanged optimized binaries; no build/profile/archive
compression competed. Lightweight summary extraction occurred between stages.

## Reproducible evidence

[SQL removal archive](evidence/sql-removal-candidate.tar.xz) has 1,288 verified
files (3,810,860 compressed bytes), with a SHA-256 manifest. It contains all 204
candidate and 36 confirmation samples, runner, plan, criteria hash, executable
identity, build/test logs, summaries and decision script. Extract alongside the
existing evidence archives. Run `python3 decide-sql-retention.py` from its documented
workspace paths (or adjust the root in the script) to reproduce the decision.
[Plain pair inputs](evidence/pair-inputs/index.json) expose counters and source
hashes without extracting large wire histories; the exporter verifies every input
and status against archive manifests. Original raw evidence remains in the cloud
workspace. Existing reports retain negative oracle tests and all adverse runs.

Local validation of the chosen production code: 22 Source tests, 13 observation
tests, 366 durable tests excluding four expensive scale cases, and all-target,
all-feature Clippy with warnings denied passed. The preceding SQL stage also
passed all 370 durable cases. Final exact-head FULL and Merge verification are
required separately; earlier green runs do not cover this final evidence head.
