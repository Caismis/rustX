# macOS FULL job budget correction

The FULL run [39068563822](https://github.com/Caismis/rustX/actions/runs/39068563822) at `49c414395fbf28151b04512b1a08348827ad3cc9` did not pass. Its macOS full job was cancelled when the 70-minute job allowance expired; Merge verification then failed because a selected FULL lane had not succeeded. All other selected lanes passed. This is not an assertion failure or a completed macOS test run.

The [check annotation](evidence/ci-budget/timed-out-annotations.json) states: “The job has exceeded the maximum execution time of 1h10m0s”. The [job timeline](evidence/ci-budget/timed-out-job.json), [compressed raw log and Cargo reports](evidence/ci-budget/manifest.json) retain the evidence, including decompressed SHA-256 hashes. The native timing artifact is [12116072494](https://github.com/Caismis/rustX/actions/runs/39068563822/artifacts/12116072494).

| Phase | Observed wall | Outcome |
|---|---:|---|
| Start to external compilation | 2m16s | Setup completed |
| External targets and real executables | 19m20s | Compilation completed |
| Unit/in-crate harnesses | 38m43s | Compilation completed |
| Unit/in-crate execution | 10m04s | Cancelled, incomplete |
| External execution and lane coverage | — | Skipped after cancellation |

Cargo's unit reports isolate 576.81 seconds for the normal rustX library and 2306.18 seconds for its `cfg(test)` library harness. This was a partial dependency-cache match, not an exact warm build: dependencies were rebuilt, and the established cache policy always rebuilds workspace crates. Both compile commands finished successfully. Their peak RSS values were 3,301,146,624 and 3,215,605,760 bytes. Serialized compiler execution is deliberate to avoid concurrent library peaks approaching runner memory; `CARGO_BUILD_JOBS=1`, debug metadata levels, cache policy and the existing `CARGO_INCREMENTAL=0` remain unchanged.

Tests were making progress: the indexed occupancy scale case passed at 22:03:03 UTC, and the last recorded passing request-fragment case finished at 22:04:44 UTC, eight seconds before cancellation. Manifest 100/300/1000 and request-fragment 300-pair scale cases had reported running for more than 60 seconds; the log does not prove they completed. There is no recorded assertion failure in this log. This supports budget exhaustion, but cannot establish that every unfinished test would pass or rule out all test regressions.

The only CI behavior change is `rust-platform-boundaries.timeout-minutes: 70 → 100`. About 60 minutes had elapsed before test execution. The historical successful run [39015122730](https://github.com/Caismis/rustX/actions/runs/39015122730), referenced by the former workflow comment, spent 12m03s on unit/in-crate execution, 2m55s on external targets and 15s on coverage. Those are a budgeting reference, not a matched performance baseline: source, runner/cache conditions and workload can differ. Combining the current observed setup/build span with that approximately 15-minute execution tail requires about 75 minutes; 100 minutes leaves roughly 25 minutes for execution/build variance and finalization. The new cap remains finite and must be validated by a complete run.

No individual deadline, assertion, selector, suite, concurrency, durability guarantee, benchmark harness, raw benchmark sample or derived accounting is changed. No benchmark rerun is needed. Focused validation parses both workflow versions and proves that only this job timeout differs semantically, runs the existing verification-policy tests, and checks whitespace. Final-head FULL plus Merge verification and independent re-review are still required; the cancelled run cannot satisfy them.
