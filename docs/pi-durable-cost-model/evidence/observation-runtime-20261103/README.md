# Observation boundary evidence

Measured Rust baseline: `03ec15b4bccccc9459fd1ec7b7fdd1bf4673a617` (schema 88). Candidate production source: `09e388f85c3b1605e862fc840ccc7a7e89a75e1a` (schema 89).

[Report](../../2026-11-03-observation-boundary.md) · [Every cell and adverse pair](../../2026-11-03-observation-boundary-tables.md).

- 2,363 distinct tests passed, zero failed, one existing ignored; Fmt and release/all-target/all-feature Clippy deny-warnings passed. Final 16-command queue: `checks-final-2`. Correctness uses release dependencies/root O0/all features and does not supply performance products.
- Ordinary clean O3/default-feature baseline and candidate binaries are separately identified. Candidate src tree: `a7641a3fb89a5a299f2bdb4bf93bddb92502986d`; candidate executable SHA256: `829d100e21ac52589ea46d1239429097e0073f84ab71482577e87654eadb5883`. 966 source/test/Cargo inputs agree between final checks and candidate build.
- 96 formal Rust products, 48 pairs, 1,128 matched requests; four pairs per case/mode, balanced order, CPUs 2,8. Every raw value and reversal is retained. This small descriptive cohort does not establish confidence intervals or Pi parity.
- 32 separate public-API inspection products: small, 10KiB leaf, 32KiB segmented, concurrent. Each writes 64 events, checks an old field and page, repeats both 32 times, and checks reopen; concurrent adds 32 writes/64 held-cut reads. First-read timings do not reset the OS page cache. Full CPU has 10ms granularity. All read-only phases write zero bytes; concurrent writes remain in full-cycle counters.
- Eight separate column/WAL products. Source column bytes fall 19,968→8,448 per 32 warm turns; main write commits stay 160. Physical process writes are essentially unchanged; WAL frames are 5,665→5,664 and 6,387→6,391. Column byte reductions are not device write savings.
- No Pi or DeepSeek products were run and no new cross-batch ratios are calculated. Historical references and adverse results remain unchanged.

`raw.tar.xz` contains **994 regular payloads**, **4,781,748 compressed bytes**, SHA256 **`cb986c55b76c1f1debeae4ad77a1399ca7603f304d73e319493e7852bc77dd24`**. `manifest.json` records each member length/hash. It includes source patch and identities, all successful/failed checks and logs, original frozen scripts, raw formal/provider/observer records, column snapshots, WAL traces/strict parser output, inspection source/results/databases, accepted audit and rendered-table data. It excludes private user directions, baseline checkout, executables, rlibs, bytecode and parser-selftest symlinks. Original runtime binary identity is a retained build record when replaying this archive; absent binaries are not re-executed or freshly verified.

The measurement-time auditor omitted an executable-identity argument in its diagnostic call and stopped with KeyError after the formal calculation. Its frozen script and failure remain. `audit-results-corrected.py` only supplies the already-frozen executable identities in that one call; no formulas, product data, WAL parser, source or measurement protocol changed. `postmeasurement-corrections.json` records original/corrected SHA256 and the exact single replacement, plus the corresponding replay entry-point replacement. The original frozen script hashes remain unchanged. All 136 product runs succeeded; no product was rerun to fix the audit.

Earlier schema-fixture/attack-fixture/Clippy failures and the standalone probe's wrong-toolchain compile failure are retained separately and never counted as final successful checks. The probe was compiled from the identical source against each ordinary O3 library; no source change accompanied the toolchain retry. The WAL parser was checked before new measurement against four retained earlier traces; those old products were not rerun.

Portable replay verifies every archive member, extracts only safe regular paths to a temporary directory, invokes the explicitly corrected read-only auditor and checks that every recomputed value equals the retained strict audit (apart from its raw-only flag). [Recorded replay](portable-verification.json) passed. It never executes products or builds, and never claims to verify current source or absent binaries.

```sh
python3 replay.py --archive raw.tar.xz --manifest manifest.json --output replay-new.json
```

Full repository CI/browser E2E and a main merge were not performed. Existing main conflicts remain; the PR stays draft.
