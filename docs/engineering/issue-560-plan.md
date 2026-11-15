# Issue #560 Implementation and Measurement Plan: Adopt Rust 1.100 and Streamline Dependencies, Module Boundaries, Tests, and CI

This is the historical Stage 0 bounded plan. Its completed implementation, measurements, negative results and final validation procedure are indexed by the [final report](../evidence/issue-560-final-report.md).

Status: **Accepted Bounded Stage 0 Plan (Revised Prior to B0/B1/B2 Execution and Stages 1–5)**

---

## 1. Base Identity, Environment, and Prerequisites

### 1.1 Repository & Toolchain Identity
- **Worktree**: `/home/caismis/Documents/codes/rustX-issue-560` (branch `issue-560-rust-1.100`; primary `/home/caismis/Documents/codes/rustX` at `c076354d6` remains untouched)
- **Base commit**: `33234e5b19f9074924a5fba1315c23dcf7210c32`
- **Locked manifest & lockfile hashes (SHA-256)**:
  - `rust-toolchain.toml`: `c910997cb152c6dc8ed13b1fdf9fa28a4ed30d6776123e27dd6170e5c66067a0`
  - `Cargo.toml`: `f9eebc35fbc37608da281746e87228f6e19bcadd0b90c503e66db64345ceca3b`
  - `Cargo.lock`: `d5065bbe2d9e8308393d92f6b3fee8ef569c9e16f646392b73aa9211be5ecc74`
- **Baseline toolchain (`B0`, pinned)**:
  - `rustc 1.98.1 (48a229cea 2026-09-01)` (`x86_64-unknown-linux-gnu`, `LLVM 22.1.8`)
  - `cargo 1.98.1 (797e8a9bc 2026-08-05)`
  - `clippy 0.1.98 (48a229ceae 2026-09-01)`, `rustfmt 1.9.0-stable (48a229ceae 2026-09-01)`
- **Target toolchain (`B1`/`B2`, installed via `rustup toolchain install 1.100.0 --profile minimal --component clippy,rustfmt`)**:
  - `rustc 1.100.0 (0ef1bc386 2026-11-09)` (`x86_64-unknown-linux-gnu`, `LLVM 23.1.5`)
  - `cargo 1.100.0 (3acc79c0c 2026-11-05)`
  - `clippy 0.1.100 (0ef1bc386 2026-11-09)`, `rustfmt 1.11.0-stable (0ef1bc386 2026-11-09)`

### 1.2 Host Resources & Prerequisites
- **Hardware / OS**: Linux `x86_64`, 16-core `Intel(R) Core(TM) Ultra X7 358H`, `30Gi` RAM + `8.0Gi` swap; `/home` `577G` free / `952G` total; `/tmp` `tmpfs` `12G` free / `16G` total. Compilation `target-*` directories live on disk-backed `/home`, never on the `16 GiB` `tmpfs`.
- **Native / Python / Node tools verified**:
  - `uv 0.11.12`, system `python3` (`3.15.0`), synced `test-support/fake-provider/.venv` (`CPython 3.12.13`)
  - `node v24.21.0`, `pnpm 11.25.0`, `/usr/bin/clang`, `/usr/bin/docker`, `/usr/bin/time`, `/lib64/libclang.so.23.1`
- **Local Node prerequisites (`tui/` and `web-console/`)**:
  - A fresh worktree does not contain `tui/node_modules` or `web-console/node_modules`. Installing locked Node dependencies via `pnpm install --frozen-lockfile` in `tui/` and `web-console/` is a test-environment prerequisite, not a source defect.
- **Platform availability boundary**:
  - This host runs Fedora Linux `x86_64` without macOS or AppArmor; macOS (`desktop-macos`, `rust-platform-*`) and AppArmor-specific kernel checks cannot be run locally and must be reported explicitly as unavailable locally until hosted FULL CI runs.
- **Memory & raw-log discipline**:
  - Bounded parallelism (`CARGO_BUILD_JOBS=4`) prevents OOM on `30 GiB` RAM, and formal performance samples never run concurrently with compilations. Raw artifacts remain outside git under `/tmp/rustx-560-control/`.

---

## 2. Rust 1.100 Adoption Contract, Build-Resource Repair, and Diagnostic Baseline

### 2.1 Explicit `python-guard` Ownership Contract (Eliminating `build.rs` and `src/tools/profile_dir.rs`)
- **Observed Rust 1.100.0 blocker**:
  - Cargo `1.100.0` enables the new `build/<pkg>/<hash>/out` `build-dir` layout by default, causing `cargo +1.100.0` `clippy` and `doc` invocations to panic inside `build.rs` -> `src/tools/profile_dir.rs` (`OUT_DIR` `/.../target/b1-probe/debug/build/rustx/7d6f99552a8e9c92/out` had 4 parents to `<profile>` instead of 3, triggering `assertion 'left == right' failed: left: Some("rustx"), right: Some("build")`).
  - Note: earlier `--timings=json,html` probes on both `1.98.1` and `1.100.0` were rejected by Cargo CLI parsing (`--timings` takes no value), before any build-script execution.
- **Architectural repair (Stage 1 — no `OUT_DIR` ancestor discovery)**:
  - `build.rs` only copied static `assets/python-guard/{python,python3}` into `<profile>/python-guard` and `<profile>/deps/python-guard` outside `OUT_DIR`, and no CI, packaging, or `web-console` script depends on Cargo-staged `python-guard` directories.
  - Eliminate `build.rs` and `src/tools/profile_dir.rs` rather than replacing a 3-parent traversal with a 4-parent traversal or searching ancestors.
  - Retain the authoritative embedded-byte (`GUARD_PYTHON`, `GUARD_PYTHON3` via `include_bytes!`) and runtime resource owner in `src/tools/workspace_python.rs`:
    1. Prefer workspace-scoped or fallback runtime guard directories written from embedded bytes and validated before use (`temp_dir`, `XDG_RUNTIME_DIR`, `/dev/shm`, executable directory). Independent final review corrected this plan's original assumption of atomic staging: the retained installer uses create-new/write/validate on final filenames; its inherited concurrent-install availability limitation is disclosed in the final report.
    2. When no writable guard directory exists, preserve `provisioned_guard()` / `guard_beside_executable()` checking an explicitly pre-provisioned read-only `<current_exe().parent()>/python-guard` sidecar (validated for executable bits and exact embedded byte equality), supporting relocated/read-only deployments without ever consulting the source/build tree.
    3. Fail closed (`guard_unavailable = true`) when neither writable runtime storage nor a valid executable-adjacent pre-provisioned guard exists, blocking system `python`/`python3` lookup via `PATH` while keeping unrelated commands available.
  - Update `docs/execution-environment.md` to document this explicit deployment contract cleanly (removing the obsolete claim that `cargo build` populates `python-guard` next to binaries), and prove relocated read-only executable-adjacent guard lookup and fail-closed behavior with behavioral subprocess/relocated-binary tests.

### 2.2 Toolchain, MSRV, Edition, and Consumer Pins (Stage 1)
- `rust-toolchain.toml`: `channel = "1.100.0"`, `profile = "minimal"`, `components = ["clippy", "rustfmt"]`.
- `Cargo.toml`: `edition = "2024"` retained, `rust-version = "1.100"` (remove obsolete `1.92` / Issue #48 MSRV comment).
- Keep `Cargo.lock` (`d5065bbe2d9e8308393d92f6b3fee8ef569c9e16f646392b73aa9211be5ecc74`) unchanged in Stage 1 (`B1`) unless an unavoidable targeted incompatibility requires a documented fix.
- Update all active toolchain pins and references across `.github/workflows/ci.yml` (all 11 `toolchain: "1.98.1"` pins), `CONTRIBUTING.md:92`, `web-console/scripts/browser-artifact.ts:69`, and `web-console/test/browser-verification.test.ts:11, 223`.

### 2.3 Strict Rustdoc & Doctest Baseline (`B0`)
- **`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked` (`target/b0`)**:
  - Failed on `B0` (`1.98.1`, exit `101`, `0:57.28`, `4,869,004 KB` Max RSS) with **106** diagnostic errors across 4 distinct categories:
    - `53` `rustdoc::private_intra_doc_links`
    - `33` `rustdoc::unresolved_intra_doc_links`
    - `19` `rustdoc::redundant_explicit_link_target`
    - `1` ambiguous intra-doc link (`` `write` is both a function and a macro ``)
  - Repairing these pre-existing rustdoc diagnostics in a distinct Stage 2 commit keeps compiler-upgrade causality (`B1`) separate from cleanup (`B2`).
- **`cargo test --workspace --doc --all-features --locked` (`target/b0`)**:
  - Passed on `B0` (`9 passed; 0 failed` in `0.54s` test exec; `2:50.26` total build+run; `8,195,180 KB` Max RSS). All 9 are `compile_fail` doctests (`src/tools/todo.rs:958`, `src/message/types.rs:211, 519`, `src/conversation/mod.rs:203`, `src/local_runtime/session_controller.rs:129, 132, 138, 144`, `src/context/status.rs:392`), currently unowned in CI.

---

## 3. Bounded Dependency, Feature, and Suppression Cleanup (Stage 2)

| Item | Current Spec / Scope | Usage & Graph Evidence | Proposed Stage 2 Action | Retained Contract / Reason | Validation Scope |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `grep-matcher` | `"0.1.9"` in `[dependencies]` | `0` direct references in `src/`, `tests/`, `examples/`; only transitive via `grep-regex` & `grep-searcher` | **Remove** direct edge from `Cargo.toml` | Transitive dependency still compiled by `grep-regex`/`grep-searcher`; removes unused direct API edge only | `grep` tool tests, default & `--all-features` checks |
| `nix` (`[dev-dependencies]`) | `{ version = "0.31.3", features = ["signal", "process", "fs", "dir"] }` | Strict subset of `[dependencies] nix` (all 11 features verified used in `src/` under Linux/macOS cfgs) | **Remove** duplicate `[dev-dependencies] nix`; keep `[dependencies] nix` | Retains every OS/process/inotify/kqueue/socket/termios/resource capability | Default & `--all-features` builds, Linux/macOS tests |
| `tower` | `{ version = "0.5", features = ["util"] }` | `src/model/adapter/openai/client.rs` uses only `tower::Service` and `tower::BoxError`; `0` uses of `tower::util` | **Narrow** direct spec to `tower = { version = "0.5", default-features = false }` | `async-openai`, `reqwest`, and `axum` still enable `tower/util` transitively, so narrowing does not drop transitive compilation | OpenAI adapter & provider tests |
| `serde_yaml` | `"0.9"` in `[dependencies]` | Used in `workflow_file`, `workflow`, `schemas`, `skills/package`, `launch_tests`, `examples/check_test_lanes.rs`; `serde_yaml_ng` is already in `Cargo.lock` via `xberg` | **Evaluate carefully** before replacing: verify `serde_yaml::Deserializer` expansion/alias budget hooks (`src/workflow_file/expansion.rs`), error wording/categories, and workflow authoring parity; no crate alias or wrapper | Retain `serde_yaml` if any error/security-budget or YAML 1.1/1.2 edge differs | `workflow_file` expansion & canonical tests, `contracts`, `check_test_lanes` |
| `mcp-fixture` | `["rmcp/server", "rmcp/transport-io", "rmcp/transport-streamable-http-server"]` | Gates test-fixture MCP servers (`src/tools/mcp/fixture.rs` + boundary/tools tests) | **Keep** explicit `mcp-fixture` feature contract as-is | Preserves feature boundary instead of distorting defaults for benchmark convenience | MCP boundary & tool tests |
| `minijinja`, `async-openai`, `jsonwebtoken`, `tokio-util`, `schemars`, `uuid`, `getrandom`, `rusqlite`, `serde_json`, `reqwest`, `rmcp`, `imbl`, `zip`, `xberg` | Intentional domain/security/serialization features | Every declared feature verified actively used (`set_fuel`, `tojson`, `create_stream_byot`, `jwt decode/encode`, `AbortOnDropHandle`/`LinesCodec`, `chrono04`, `v7`, `getrandom::fill`, SQLite hooks, lossless `float_roundtrip`/`raw_value`, `rustls`) | **Keep** verbatim | Preserves SQLite durability, TLS, serialization precision, isolation, and parser limits | Covered by standard workspace suite |

### Diagnostic & Suppression Audit (893 `allow(...)` tokens: 713 `src/`, 171 `tests/`, 9 `examples/`)
- Remove confirmed dead items behind `#[allow(dead_code)]` (`31` in `src/`, `10` in `tests/`), verifying serde wire fields (`src/model/adapter/anthropic/wire.rs`) and schema/test helpers before pruning or gating under `#[cfg(test)]`.
- Narrow file-level `#![allow(...)]` blankets (`src/runtime/permission/bash/mod.rs`, `src/durable/sqlite/auxiliary.rs`, `src/local_runtime/launch_tests.rs`, and test-crate `too_many_lines` blankets) where natural structure permits without fragmenting cohesive state machines.
- Retain all `12` `src/` and `1` `tests/` `#[allow(unsafe_code)]` OS/fd/signal/prctl/seccomp sites and justified `#[allow(clippy::large_futures)]` annotations with narrow scope and semantic comments.

---

## 4. Bounded Ownership-Based Module Refactor Plan (Stage 3)

Module splitting improves ownership clarity and maintainability; it does **not** by itself create Cargo compilation boundaries or guarantee compile-time/RSS gains. Every split uses descendant module privacy rather than widening private items to `pub(crate)` or `pub`.

### 4.1 Selected Bounded Production Targets (3 Files) + Cohesive Test Organization
1. **`src/tools/workspace_python.rs` (`2067 prod / 5293 total`)** *(directly supports Section 2.1 build-resource repair)*:
   - Extract cohesive child modules for guard resource resolution/validation/installation (`guard`), pin/requirement parsing & interpreter probing (`interpreter`), and keep singleton `Owner` state/locks/permits/gates cohesive in `owner` (or parent module facade) + external `tests.rs` submodules.
2. **`src/local_runtime/session_controller.rs` (`1575 prod / 2823 total`)**:
   - Extract private child modules for coherent `reads` (session/history/statistics/trace/preview repair), `uploads` (archive/upload lifecycle), `lineage` (node/tree/clone/fork/copy), and `settings` operations while keeping `SessionController` (`catalog` + `preparation` locks, commit authority, and public/doctest surface) intact.
3. **`src/runtime/workflow.rs` (`4941 prod / 7273 total`)**:
   - Extract cohesive `compiler` (`SchemaMap`/`SchemaProofStep` validation & lowering) and `catalog` (`WorkflowCatalog` admission/lookup) child modules while keeping live runtime execution, output latches, and state transitions together rather than spreading the state machine.
4. **Targeted test-only organizational cleanup**:
   - Extract cohesive `dispatcher/tests.rs`, `config/tests.rs`, and coherent groups within `src/local_runtime/launch_tests.rs` where separation reduces review noise. Do **not** mechanically split every >500-line inline `#[cfg(test)]` block across the repository.

### 4.2 Reviewed & Retained Large Files (Out of Bounded Refactor List)
`src/tools/background.rs` (`3062/6097`), `src/capabilities/coordinator.rs` (`2898/5015`), `src/runtime/interaction.rs` (`2667/4976`), `src/runtime/recovery.rs` (`2354/3171`), `src/model/catalog.rs` (`2187/3136`), `src/runtime/subagent/process.rs` (`2132/3577`), `src/tools/native/bash/supervisor.rs` (`1325/2167`), `src/runtime/interactive_process.rs` (`953/2111`), and `src/app_server/protocol.rs` (`2175`) were reviewed and are retained intact unless a directly necessary compiler/diagnostic fix touches them.

---

## 5. Test Disposition Map, Determinism, and Doctest/CI Ownership (Stages 4–5)

### 5.1 Initial Disposition Map (Populated & Expanded During Stage 4 Implementation)
| Original test/group | Invariant / failure boundary | Action | Surviving proof or explicitly removed contract | Final compiled owner | CI lane |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `workspace_python::tests::no_guard_directory_rejects_path_lookup_and_keeps_other_commands` (`src/tools/workspace_python.rs:3788`) | When all writable storage fails, a pre-provisioned read-only executable-adjacent `python-guard` blocks `python3` without consulting the build tree; if absent, native Bash fails closed (`guard_unavailable = true`) | **Replace `build.rs` fixture reliance** with a behavioral relocated-binary subprocess test (`TempDir` containing symlink/copy of test runner + read-only `0o555` `python-guard/`) plus fail-closed check when absent | Proves real `current_exe()` sidecar verification & fail-closed rejection; explicitly removes `cargo build`-staged `python-guard` sidecar contract | `tools::workspace_python::tests` (`--lib`) | `rust-contracts`, `rust-fast` |
| `tests/tools/bash.rs:405-414` (`marker.exists()` `200 * 10ms` negative poll) | Group cancellation kills shell + backgrounded descendant before `tool` returns | **Replace unconditional 2.0s poll** with deterministic process/pipe-EOF/FIFO witness proving descendant exit before parent return | Same group-termination proof without 2.0s unconditional wait on passing runs | `tests/tools/bash.rs` (`--test tools`) | `rust-boundaries` |
| `src/tools/native/bash/tests.rs` & `tests/tools/bash.rs` readiness/lifecycle polls (`10ms` sleeps) | Process spawn readiness before cancellation / orphan-group cleanup | **Consolidate** shared readiness synchronization (FIFO/pipe/sentinel or `Notify` where in-process); keep bounded PID `kill(..., 0)` OS exit checks where external kernel reap is under test | Preserves exact signal/process-group race interleavings cleanly | `tools::native::bash::tests` / `tests/tools/bash.rs` | `rust-contracts` / `rust-boundaries` |
| `tests/provider/transport.rs:572, 634, 696` (`5ms` `attempt_count()` poll loops) | Provider connection accepted before cancellation / retry transition | **Replace** poll sleeps with deterministic barrier/`Notify` waiter on fake server accept | Deterministic linearization proof before cancellation fires | `tests/provider/transport.rs` (`--test provider`) | `rust-contracts` |
| Inline `compile_fail` doctests (`todo.rs`, `types.rs`, `conversation/mod.rs`, `session_controller.rs`, `status.rs` — 9 total) | Public API / type-level visibility restrictions fail to compile | **Audit imports/symbols** + add positive API visibility/import unit checks so broken imports cannot pass as intended privacy failures | Intended `compile_fail` contract verified both negatively (`--doc`) and positively | `cargo test --doc` + unit tests | `rust-contracts` (`check_test_lanes` enforced) |
| Oversized integration fixture setup (`tests/boundary/subagent/conformance.rs`, `tests/scripted/context/runtime_integration.rs`, `tests/durable/publication.rs`) | Multi-boundary agent/durable/context semantics | **Share read-only setup builders** cleanly while keeping writable SQLite/workspaces/processes isolated per test | Preserves every behavioral assertion & counterexample matrix | Original `--test` owners | `rust-contracts` / `rust-boundaries` |

### 5.2 Explicit CI Owners & Real Redundancy Targets (Stage 5)
1. **Explicit diagnostic & test ownership across CI**:
   - **Default-feature executable & typecheck path**: `quality` runs `cargo check --workspace --all-targets --locked` + default-feature binary verification (`cargo build --bins --locked`) alongside `--all-features`; default strict Clippy / `--all-features` strict Clippy have explicit execution in `quality`.
   - **Strict rustdoc**: `quality` owns `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked`.
   - **Doctests**: `rust-contracts` owns `cargo test --doc --all-features --locked` (or `--workspace --doc --all-features --locked`); extend existing `examples/check_test_lanes.rs` (`parse_command`, discovery, `REQUIREMENTS`) to parse `--doc` and enforce doctest ownership plus updated module selectors after Stage 3 moves.
2. **Real redundant build inside `rust-contracts` (`ci.yml:311-327`)**:
   - In `B0` CI (`run_id: 43659074269`), `rust-contracts` took `2378s` total: `215s` compiling default-feature `cargo build --bins` (`dev` profile) followed immediately by `1907s` compiling+running `cargo test --lib --bins --examples --all-features` (`[profile.test]` + `mcp-fixture`). Meanwhile `tui`, `browser-prepare`, and `app-server-protocol` build only their required default-feature artifacts (`cargo test` all-features is not re-run inside those three lanes).
   - **Stage 5 action**: test unifying `rust-contracts` bin preparation with `--all-features` and `[profile.test]` (verifying both `target/debug/` binary placement and `Librustx`/dep artifact freshness via `--message-format=json`) so `rust-contracts` builds the dependency graph and `rustx` library **once** instead of twice.
   - Measure actual compilation savings (around the `~215s` prep + duplicate `librustx` step inside `rust-contracts`, not an ungrounded overall job percentage) while preserving separate ordinary-library vs test-harness verification and macOS/browser/FULL gate policies (`rust-cache` trusted `main` writers only).

---

## 6. B0 / B1 / B2 Measurement Protocol

### 6.1 Historical vs Comparable Local Measurements
- Historical initial `B0` invocations (`target/b0` shared across `clippy` then `doc` then `--doc`, plus `check_test_lanes` whose child `cargo test` commands omitted `CARGO_TARGET_DIR` and built in default `target/`) established baseline diagnostics (`clippy` 0 warnings, strict `doc` 106 errors, `--doc` 9 passed, `check_test_lanes` 6,248 Linux runnable tests + 9 unassigned doctests) and CI snapshot timings (`run 43659074269`, which was still running macOS full and Browser shard 2 at capture time; prior run `43650818119` failed Browser shard 3), **not** isolated cold native builds.
- Before editing the toolchain pin for Stage 1, establish clean, comparable **B0 (`1.98.1`)**, then **B1 (`1.100.0` + Stage 1 minimal)**, then **B2 (`1.100.0` + Stage 5 complete)** measurements on disk-backed `target/<chk>-cold` directories using `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL` explicitly recorded, `/usr/bin/time -v`, supported Cargo `--timings`, and raw logs under `/tmp/rustx-560-control/`:
  1. **Cold native build (`cargo build --bins --locked` and `cargo build --bins --all-features --locked`)** on a fresh target directory (distinguishing crate download cache, cold target dir, and warm OS page cache; repeated if variance warrants).
  2. **Compiled-dependency cache-hit rebuild** (touching `src/lib.rs` on an already-built `target/<chk>-cold` so deps stay cached while `rustx` rebuilds).
  3. **Incremental leaf-module rebuild** (`CARGO_INCREMENTAL=1`, repeated 3x cleanly reverting a whitespace comment in a leaf module, measuring `cargo check` + `--no-run` + single leaf unit test).
  4. **Test prep vs execution**: time `--no-run` separately from test binary execution on `--test contracts`, `--test provider`, `--test process`, `--test durable`, `--test tools`, and `--doc` (with `CARGO_TARGET_DIR` exported for subprocess-spawning tests/tools).
  5. **Representative runtime workload**: build preserved `conformance` release binaries with `benchmarks/runtime/build-revision.py` for `B0`, `B1`, and `B2` and run paired interleaved comparisons (`benchmarks/runtime/compare-revisions.py --pairs 3`, `compare-append.py`, `summarize-comparisons.py`) as regression verification.

---

## 7. Proposed Stage Commits
1. **Stage 0 (this revision)**: `Revise Stage 0 plan per commander constraints` (`docs/engineering/issue-560-plan.md`).
2. **Stage 1**: `Adopt Rust 1.100.0 toolchain and explicit python-guard ownership` (update pins/MSRV/CI/docs, eliminate `build.rs` & `profile_dir.rs`, update `workspace_python` relocated guard tests, establish `B0` vs `B1` measurements).
3. **Stage 2**: `Clean unused dependencies, features, dead code, and rustdoc errors` (strict rustdoc fixes + Cargo/suppression cleanup with traceable disposition).
4. **Stage 3**: `Decompose workspace_python, session_controller, and workflow modules` (plus coherent `dispatcher`/`config`/`launch_tests` test organization without visibility widening).
5. **Stage 4**: `Consolidate test preparation and replace polling waits with barriers` (final test disposition map + deterministic sync + `compile_fail` positive guards).
6. **Stage 5**: `Optimize rust-contracts build reuse, enforce doctest lanes, and record B0/B1/B2 verification` (CI workflow + `check_test_lanes.rs` + final local full validation & benchmark comparison docs).
