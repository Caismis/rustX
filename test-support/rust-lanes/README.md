# Rust test shards: build once, test many

Phase 2 of the CI rework: each heavy Rust lane compiles its harnesses once on
the build runner, packs them into a checked artifact, and fans the test
execution out to N shard runners. A report job then proves every discovered
test ran exactly once.

## Pipeline

```
<lane> job                       <lane>-shard i/N           <lane>-report job
─────────────────────────────    ────────────────────────   ──────────────────
prepare_native_tests.py          download artifact          download inventory
  (cargo test --no-run,          untar at compile-time       + every shard
   artifact stream saved)         paths                      report
rust_test_shards.py manifest  →  check manifest (SHA, OS,   rust_test_shards.py
rust_test_shards.py pack          arch, triple, rustc,        verify
rust_test_shards.py inventory     Cargo.lock, features)    + every shard's
upload prepared + inventory    →  check harness sha256s     result == success
                                 run --exact batches
                                 upload report.json
```

## Artifact binding (manifest.json)

An artifact is usable only when every field matches the runner that unpacks
it; `run` fails closed otherwise.

- `git_head` — the source commit (`GITHUB_SHA`/`RUSTX_SOURCE_HEAD`, else
  `git rev-parse HEAD`).
- `runner_os`, `runner_arch`, `target_triple` — platform identity.
- `rustc_version` — the toolchain that compiled the harnesses.
- `cargo_lock_sha256` — dependency identity.
- `features`, `profile` — the compilation configuration.

`pack` also records each harness binary's sha256 into the inventory; a shard
runner re-hashes before executing, so a corrupted or swapped binary is
rejected before a single test runs.

## Lane spec (`test-support/rust-lanes/*.json`)

The spec is the single declaration of what a lane executes. It is consumed by
two readers:

- `rust_test_shards.py inventory` expands each entry's `harness` selector
  (`lib:<name>`, `test:<name>`, `bin:*`, `example:*`, `bench:*`) against the
  packed binaries and runs `harness --list <libtest_args>` to enumerate the
  runnable universe, then assigns tests to shards by duration weight (LPT).
- `check_test_lanes` expands the same spec into coverage steps, so the lane
  checker's target/filter validation sees shard jobs exactly like ordinary
  `cargo test` steps.

Entry fields: `name` (free-form), `harness` (selector), `features` (the
step's feature flags, for coverage reporting), `libtest_args` (parsed with
the same rules as the `--` tail of a `cargo test` command: positive filters
and `--skip` only).

## fail-closed coverage

`rust_test_shards.py verify` reads the inventory plus every shard's
`report.json` and fails unless:

- every shard index reported (a cancelled shard is a missing report),
- every inventoried runnable test executed exactly once and ended `ok`,
- no shard ran a test outside the inventory.

The lane checker still validates at parse time that the spec's targets and
filters cover the lane's assignment, so a spec that drops a target or leaves
a stale `--skip` fails before CI ever schedules a shard.

## Why libtest shards, not nextest archives

The Codex reference workflow distributes a `cargo nextest archive` to shard
runners. We evaluated it and chose native libtest execution for this lane:

- nextest runs each test in its own process and defaults to
  `fail-fast = false` semantics per test; several suites here assume the
  libtest single-process, shared-state execution model they were written
  under (fixture servers bound per-process, `#[serial]`-style ordering
  assumptions, once-only setup in module scope). Moving them under nextest
  changes runtime semantics — exactly what this phase forbids.
- The artifact-binding and exactly-once verification machinery is identical
  either way; nextest buys per-test process isolation we neither need nor
  can afford semantically, plus a `cargo-nextest` install on every runner.
- nextest remains the right answer if a lane later wants isolation;
  `pack`/`manifest`/`verify` are harness-agnostic and could pack a nextest
  archive instead by swapping the `run` implementation.

## Duration weights

`run` records per-batch wall time in the report's `durations` map
(stable libtest has no per-test timing; `--report-time` is nightly-only).
Regenerate the weights file after a full run with:

```
python3 test-support/rust_test_shards.py weights \
  --runs <dir of shard report dirs> --out test-support/rust-shard-weights.json
```

`inventory --weights` reads it; unknown tests fall back to 1.0 s. Weights are
an optimization input only — coverage never depends on them.
