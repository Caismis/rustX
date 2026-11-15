# Issue #560 Stage 1 Disposition: Removed `profile_dir` Test and Replaced Automatic-Sidecar Contract

## Scope

This evidence record documents the architectural removal of `build.rs` and `src/tools/profile_dir.rs`, the disposition of the removed `profile_dir` synthetic unit test, and the replacement of the Cargo-staged automatic `python-guard` sidecar contract under Rust 1.100.0.

## 1. Removed Synthetic Test Disposition

- **Removed test**: `tools::profile_dir::tests::custom_profile_directories_are_parents_of_build` (previously in `src/tools/profile_dir.rs`, also included by `build.rs`).
- **Original purpose**: Asserted that Cargo's `OUT_DIR` always resided three directory levels below `<profile>` (`<profile>/build/<package>/out`), enabling `build.rs` to locate `<profile>` and copy `assets/python-guard/{python,python3}` into `<profile>/python-guard` and `<profile>/deps/python-guard`.
- **Rust 1.100.0 failure**: Cargo 1.100.0 uses the `<profile>/build/<pkg>/<hash>/out` `build-dir` layout by default, causing `build.rs` to panic on `assert_eq!(build_dir.file_name(), Some("build"))` during compilation (`left: Some("rustx"), right: Some("build")`).
- **Disposition**: **Removed permanently** alongside `build.rs` and `src/tools/profile_dir.rs`. Searching parent ancestors or changing a fixed 3-parent traversal to a fixed 4-parent traversal is rejected because `build.rs` should not write artifacts outside `OUT_DIR` or guess Cargo's internal target-directory layout.

## 2. Replaced Automatic-Sidecar Fixture Contract

- **Removed contract**: `cargo build` / `cargo test` no longer automatically copies `assets/python-guard/{python,python3}` beside compiled binaries in `target/<profile>/python-guard` or `target/<profile>/deps/python-guard` (see updated [execution-environment.md](../execution-environment.md)).
- **Retained production and deployment contract** (`src/tools/workspace_python.rs`):
  1. Embedded guard script bytes (`GUARD_PYTHON`, `GUARD_PYTHON3` via `include_bytes!`) remain compiled into the binary and are created, written, and validated in a writable workspace state directory or runtime fallback directory (`temp_dir`, `XDG_RUNTIME_DIR`, `/dev/shm`, executable parent). The inherited installer writes final filenames directly; it does not provide atomic publication. An interrupted or concurrent installation may reject that directory and select the existing fresh fallback. Independent review corrected the earlier atomic-staging description; this issue does not change that inherited availability behavior.
  2. When no writable directory is available, `provisioned_guard()` / `guard_beside_executable()` checks for an explicitly pre-provisioned `<current_exe().parent()>/python-guard` sidecar (or canonicalized executable parent), validating executable bits and exact byte equality against `GUARD_PYTHON` and `GUARD_PYTHON3`. Source/build-tree fallback is never consulted.
  3. When neither writable runtime storage nor a valid pre-provisioned sidecar exists, composition fails closed (`guard_unavailable = true`), refusing to expose an unguarded host `PATH` to native Bash while keeping unrelated native commands available.

## 3. Replacement Behavioral Proof

- **Surviving test**: `tools::workspace_python::tests::no_guard_directory_rejects_path_lookup_and_keeps_other_commands` (in `src/tools/workspace_python.rs`).
- **Behavioral coverage replacing the removed automatic-sidecar assumption**:
  - Stages an isolated relocated test executable inside temporary directories (via hard-link or copy, without `chmod`ing the shared build artifact inode) and re-executes itself using a dynamic `module_path!()`-derived `--exact` selector (remaining valid across Stage 3 module moves).
  - **Unprovisioned mode** (`RUSTX_TEST_GUARD_MODE=unprovisioned`): Verifies `provisioned_guard()` returns `None`, `finish_guard(None, ...)` sets `guard_unavailable = true`, and `execution_environment::compose(...)` returns `Err`.
  - **Provisioned mode** (`RUSTX_TEST_GUARD_MODE=provisioned`): Explicitly installs `python-guard` beside the relocated executable, sets **both** the `python-guard` directory and its `python` / `python3` scripts read-only (`0o555`), verifies `provisioned_guard_for` and `provisioned_guard()` resolve the sidecar, verifies `python3`, `/usr/bin/env python3`, `/bin/sh -c 'python3 ...'`, and `#!/usr/bin/env python3` scripts all exit `127` with the guard diagnostic while `ls /bin/sh >/dev/null && printf ok` succeeds, and deterministically restores fixture permissions even on spawn error or panic.
  - **Subprocess execution witness**: Both child invocations verify both libtest output (`1 passed; 0 failed`) and a unique per-mode witness file (`RUSTX_TEST_GUARD_WITNESS`) written at the end of the executed branch so a zero-test selector cannot pass silently.

## 4. Explicit Companion Binary Sibling Contract and Native Test Preparation (`B1`)

- **Rejected pattern**: Guessing Cargo's internal layout (`<profile>/deps` or `<profile>/build/<pkg>/<hash>/out`) from `std::env::current_exe()` inside `src/runtime/process_runner.rs` (`companion_binary`).
- **Accepted production contract**: `companion_binary(name)` in `src/runtime/process_runner.rs` resolves `bash-supervisor`, `interactive-supervisor`, and `workflow-renderer` exclusively beside `std::env::current_exe().parent()` with `std::env::consts::EXE_SUFFIX`. Missing companions fail closed at the resolved sibling path without searching ancestor directories or stale build outputs.
- **Native test preparation owner** (`test-support/prepare_native_tests.py` + `test-support/test_prepare_native_tests.py`):
  - Executes `cargo test ... --no-run --message-format=json` (and builds `cargo build --bins --message-format=json` with matching flags, injecting `--profile=test` when no `--release`/`-r`/`--profile` was selected so Cargo matches `[profile.test]` `debug = "line-tables-only"`, if a `--lib`/`--bins`/`--examples` selector omitted integration `--test` targets), relays rendered compiler diagnostics to stderr without masking failures, and fails closed on non-zero exit, missing/unsuccessful `build-finished`, incomplete binary targets, foreign-checkout `manifest_path`/`src_path`, missing root ordinary `lib` artifacts (`profile.test == false`), or mismatched features/`profile_signature` (including `debuginfo`, `opt_level`, `debug_assertions`, `overflow_checks`).
  - Supported CLI contexts (separated standard Cargo flags): `--locked`, `--frozen`, `--offline`, `--manifest-path`, `-p`/`--package`, `--target`, `--target-dir`, `--release`/`-r`/`--profile`, `--features`/`-F`, `--all-features`, `--no-default-features`, `--config`, `-j`/`--jobs`, `--color`, `--timings`, and test target selectors (`--lib`, `--bins`, `--examples`, `--tests`, `--benches`, `--all-targets`, `--bin`, `--example`, `--test`, `--bench`); unsupported compact or unforwarded context-affecting spellings (for example `-j4`, `-Ffeat`, `--keep-going`, `--doc`) fail closed rather than being dropped.
  - Physically stages each emitted package binary beside every emitted test harness executable so production, in-crate tests, and external integration tests obey the exact same executable-sibling contract.
- **Mandatory `B1` CI build-contract repair**:
  - Updated `quality`, `rust-fast`, `rust-contracts`, `rust-boundaries`, `rust-platform-core`, and `rust-platform-boundaries` in `.github/workflows/ci.yml` and `examples/check_test_lanes.rs` (`TRANSPARENT_WRAPPERS`) to use `test-support/prepare_native_tests.py`.
  - In `rust-contracts`, replaced the standalone default-feature `cargo build --bins` step with unified all-features preparation (`--lib --bins --examples --test contracts --test provider`), giving unit/contract tests matching `--all-features` companion binaries while eliminating the duplicate ordinary-library compilation as part of the mandatory `B1` build-contract repair (not deferred or attributed solely to `B2`).
