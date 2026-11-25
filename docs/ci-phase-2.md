# CI Phase 2 — build once, test many, parallel execution

Evidence and architecture for the second CI optimization phase (draft PR #610,
base `devin/1794960987-macos-ci-build-speed`). The mechanism lives in
`test-support/rust_test_shards.py` and `test-support/rust-lanes/`.

## What changed

- **Coverage-check recompilation eliminated (A).** `check_test_lanes` no longer
  rebuilds member libtest harnesses per member: it reuses Cargo's own
  `--no-run` artifact stream under each FULL step's real package selection
  (`cargo test --locked --lib <selection> <features> --no-run
  --message-format json`), matches each member to its already-compiled
  harness by package id + lib src fingerprint, and only falls back to
  bare `-p` enumeration for members no FULL step covers. The ~8-minute
  member rebuild the coverage check used to pay is gone.
- **CARGO_BUILD_JOBS experiment (B).** `rust-contracts` and
  `rust-boundaries` run `CARGO_BUILD_JOBS=4` (Linux 4 vCPU runners).
  Compiler-budget steps record the value, `nproc` and memory each run.
- **Build once, test many (C).** `rust-contracts` and `rust-boundaries`
  compile their harnesses once via `prepare_native_tests.py` (the
  artifact JSON stream is captured in-line via
  `RUSTX_PREPARED_JSON_OUT`), then:
  `manifest` (binds head SHA, OS, arch, triple, rustc, Cargo.lock,
  features, profile) → `pack` (repo-relative layout so compile-time
  `CARGO_BIN_EXE_*` paths resolve) → `inventory` (libtest discovery under
  the lane's own spec filters + LPT shard assignment) → upload.
  N shard runners verify the manifest, re-hash every harness, execute
  `--exact` batches, and upload per-test reports. `<lane>-report`
  proves exactly-once coverage and all-success.
- **Duration-weighted deterministic sharding (D).** LPT over
  `test-support/rust-shard-weights.json` (regenerate from completed
  reports via `rust_test_shards.py weights`); unknown tests weight 1.0.
- **Cache + artifact reuse (E).** Same-workflow reuse is the primary
  mechanism: one compile feeds N executors. `rust-contracts-post`
  restores the lane's `rust-cache` lineage (shared-key
  `rust-contracts`). PR #590's measured cache data:

  | Variant | Cold | Hot |
  |---|---|---|
  | rust-cache deps only (kept) | 28:23 | 22:32 |
  | rust-cache + workspace crates | 25:42 | 12:51 |
  | sccache (GHA backend) | 23:02 | 13:01 |
  | rust-cache + sccache | 22:15 | FAIL @50m |

  sccache stays a documented candidate for the build jobs (stable
  13–15 min hot) but is not adopted here: it needs its own cache
  namespace and lane changes, and this phase's win comes from removing
  redundant compilation, not caching it.

## Fail-closed guarantees

- Artifact unusable unless manifest matches: git head, OS, arch, target
  triple (checked against the runner's machine), pinned toolchain channel
  (checked against `rust-toolchain.toml` — no rustc invoked), Cargo.lock
  sha256, features, profile. Every harness's sha256 is re-verified before
  execution.
- The report job requires every shard to report, every inventoried test
  to run exactly once, all results `ok`, and `needs.<lane>-shard.result
  == success`. Missing/duplicate/failed/cancelled coverage fails.
- `check_test_lanes` expands each lane's shard spec into coverage steps
  (`rust_test_shards.py run --spec`), so the same target/filter
  validation that guards ordinary `cargo test` steps guards shard legs;
  each sharded lane's check names both the lane job and its shard job.
- Member crates, doctests, and the lane check itself keep running
  (`rust-contracts-post`, parallel to shards) — nothing was moved to
  nightly, skipped, or weakened.

## Deterministic regression coverage

`test-support/test_rust_test_shards.py` (13 tests): wrong/expired
artifact identity, foreign lockfile, tampered harness, feature-config
mismatch, cross-platform manifest, package-selection change (spec names
an absent harness), missing shard report, duplicate execution,
incomplete execution, shard failure recorded + surfaced, unexecuted
test-name rejection, missing harness.

## Topology

```
rust-contracts / rust-boundaries      <lane>-shard ×3          <lane>-report
  prepare (one compile, JSON out)  →    checkout                verify:
  manifest → pack → inventory →        manifest + sha256 check   every test
  upload prepared + inventory          run --exact batches    →   exactly once
                                       report.json upload        all ok
rust-contracts-post (parallel): doctests + member libs + lane check
```

## Measurements

Filled in from FULL runs on this branch — see the PR description and the
final report; baseline run 45356995562 for comparison (FULL 78m43s,
contracts 70m20s, coverage check 8m26s).
