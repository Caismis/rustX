# Issue #560 Stage 5 CI, Lane Ownership, Cost & Reproducibility Disposition

This document records the Stage 5 CI hardening, lane-checker enforcement, benchmark-driver portability repair, breaking deployment contract, and consolidated verification/reproducibility map for Issue #560 on top of Stage 1–4.

- Prior stage maps: [Stage 1 Guard Disposition](issue-560-stage1-guard-disposition.md), [Stage 2 Cleanup Disposition](issue-560-cleanup-disposition.md), [Stage 3 Module Disposition](issue-560-stage3-module-disposition.md), [Stage 4 Test/Synchronization/Surface/Benchmark Disposition](issue-560-stage4-test-disposition.md).
- Plan & policy: [Issue #560 Bounded Plan](../engineering/issue-560-plan.md), [Repository Policy](../repository-policy.md), [Test Architecture](../../tests/README.md).

## 1. Breaking Build & Deployment Contract (Rust 1.100.0)

1. **Pinned toolchain (`rust-toolchain.toml`)**: `channel = "1.100.0"`, `profile = "minimal"`, `components = ["clippy", "rustfmt"]`. All CI Cargo build, test, check, clippy, doc, run, and metadata steps use `--locked` (`cargo fmt` itself takes no `--locked` flag; this does not cover all historical contributor commands outside CI and managed generator/checker scripts).
2. **Removal of `build.rs` and `src/tools/profile_dir.rs`**:
   - Cargo `1.100.0` defaults to `<profile>/build/<pkg>/<hash>/out` `OUT_DIR` nesting, breaking the historical parent-depth walk that copied `python-guard` during Cargo builds.
   - `companion_binary` sibling resolution now locates `bash-supervisor`, `interactive-supervisor`, `workflow-renderer`, and companion binaries directly alongside `std::env::current_exe()`. Deployment/packaging must place sibling executables directly in the binary directory (no build-tree or `OUT_DIR` probing).
   - Native test execution requiring companion binaries (`rust-fast`, `rust-contracts`, `rust-boundaries`, `rust-platform-core`, `rust-platform-boundaries`, and full local `--all-targets` runs) must run `python3 test-support/prepare_native_tests.py -- cargo test ... --locked --no-run` first so Cargo compiler-artifact executables are validated and staged beside every emitted test harness.
3. **Runtime / Benchmark Contract Updates**:
   - `app_server::command::Commands::start` releases `Mutex<State>` before publishing via `watch::Sender<u64>`, resolving the `Commands` `State`/watch ABBA lock inversion demonstrated with the test-only `admitted_for_test` lock-boundary probe.
   - `agent_loop::runtime_benchmark` and `benchmarks/runtime/compare-http.mjs` enforce `urn:rustx:invocation` envelope wrapping and exact declared workspace `Current working directory` matching with zero `/unused` or capture-derived fallback.
   - `benchmarks/runtime/compare-revisions.py` and `benchmarks/runtime/controller.py` strip credential/token/secret variables via shared `child_base_env` without hardcoding `/workspace/.uv-cache`, respecting caller `UV_CACHE_DIR` or default `uv` cache resolution.
   - `tests/scripted/app_server/residency_policy.rs` (`one_reaper_timer_wakes_from_manual_clock_and_unloads_automatically`, `inactive_goal_can_idle_unload_and_policy_is_user_only`) synchronizes on `probe.status_before_wait` after `load` so background `status_observer` initial capture settles before asserting single-tick idle eviction under full-workspace load, and `tests/scripted/app_server/workflow_run_snapshot.rs` (`issue552_rpc_header_and_chunk_authorization_routing_and_warmed_cache_denial`) awaits `AttemptCompletion::observed` so `RuntimeClient` checkpoint projection publishes the completed seed turn before subsequent `SessionCheckpoint` reads.

## 2. Explicit CI Lane Ownership (`.github/workflows/ci.yml`)

No second scheduler, new job topology, weakened gate, or `save-if` relaxation is introduced; `plan` (`.github/scripts/verification.py`), `verification` merge gate, and trusted-`main`-only `Swatinem/rust-cache@v2` publishers remain intact.

| Capability / Gate | Authoritative CI Lane | Exact Command / Enforcer |
| :--- | :--- | :--- |
| Formatting | `quality` | `cargo fmt --all -- --check` |
| Strict Clippy (default features, all targets) | `quality` | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Strict Clippy (all features, all targets) | `quality` | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` |
| Strict rustdoc | `quality` | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked` |
| Benchmark & native-prep Python unit suites | `quality` | `python3 -m unittest discover -s benchmarks/runtime -p 'test_*.py' -v` & `-s test-support` |
| Benchmark request-comparison Node contracts | `quality` | Pinned `NODE_VERSION`; `node --test benchmarks/runtime/compare-http.test.mjs benchmarks/runtime/pi-oracle.test.mjs` |
| Default-feature binary builds & native consumers | `tui`, `desktop-macos`, `browser-prepare` | `cargo build --bins --locked` (`tui`, `desktop-macos`); `node scripts/browser-artifact.ts pack` (`browser-prepare`) |
| FAST Linux unit/contract & process feedback | `rust-fast` | `prepare_native_tests.py` + `cargo test --lib --all-features --locked ...` + `--test process` + `check_test_lanes -- --job rust-fast` |
| Documentation & runtime-example contracts | `documentation-contracts` / `rust-contracts` | `cargo test --test contracts --all-features --locked` |
| Full Linux unit, bin, example, `contracts`/`provider` | `rust-contracts` | `prepare_native_tests.py` + `--lib --bins --examples --all-features --locked -- --skip boundary_suites::` + `--test contracts --test provider --all-features --locked` |
| Explicit doctests (positive + `compile_fail`) | `rust-contracts` | `cargo test --doc --all-features --locked` + `check_test_lanes -- --job rust-contracts` |
| Full Linux boundary suites | `rust-boundaries` | `uv run --frozen pytest` (`fake-provider`) + `prepare_native_tests.py` + 7 external boundary targets + `boundary_suites::` + `check_test_lanes -- --job rust-boundaries` |
| Native macOS boundary core (FAST) | `rust-platform-core` | `prepare_native_tests.py` + 7 external boundary targets (`--all-features --locked`) + `check_test_lanes -- --job rust-platform-core` |
| Native macOS full unit/in-crate/external boundaries | `rust-platform-boundaries` | `prepare_native_tests.py` + `--lib --bins --all-features --locked -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::` + 7 external targets + `check_test_lanes -- --job rust-platform-boundaries` |
| Generated App Server protocol & TUI/Dev/Web/Browser | `app-server-protocol`, `tui`, `web-client`, `browser-shard`, `browser-report` | `pnpm check` (invoking `cargo run --locked --example generate_app_server_protocol`) + `pnpm typecheck`, `vitest`, i18n/provenance, Playwright shards |

## 3. `examples/check_test_lanes.rs` Enforcement & Cost Analysis

1. **Doctest Ownership & All-Block Discovery**:
   - `parse_metadata` derives `Kind::Doc` from `Kind::Lib` (`doctest != false`); `parse_command` supports `--doc` while explicitly excluding `Kind::Doc` from `--all-targets` (matching Cargo behavior).
   - `parse_doctest_list` parses all native rustdoc `--list` output blocks emitted by `cargo test --doc --locked --color never -- ... --list`, verifying every `<n> test[s], <m> benchmark[s]` block summary and the final `all doctests ran in ...` timing trailer without treating block count or grouping as kind proof (`--list` does not label block kind).
   - Semantic coverage of positive API examples and negative `compile_fail` proofs is established by explicit `cargo test --doc --all-features --locked` execution plus `REQUIREMENTS` pinning `doc` `""` and all 6 paired owner symbol namespaces (`AgentStatusSurfaceView`, `ConversationState`, `SessionController`, `AgentStatusGenerationMetadata`, `CompactionSummaryMetadata`, `ConversationTodoList`) alongside Stage 3/4 modularized/hardened namespaces (`tools::workspace_python::tests::`, `runtime::workflow::tests::`, `app_server::command::tests::`, `contracts`, `provider`, `conformance` `agent_loop::runtime_benchmark::`).
2. **Eliminated Repeated Cargo Startup Cost (`CargoDiscovery`)**:
   - For non-doc libtest targets (`lib`, `bin`, `example`, `test`, `bench`), `CargoDiscovery` previously spawned `cargo test --locked ... -- <filters> --list --format terse` on every unique `(features, target, libtest, ignored)` query (e.g., counterfactual `--skip` and per-filter probes in `rust-contracts` / `rust-boundaries` / `rust-fast`).
   - Now `CargoDiscovery::acquire_harness` runs `cargo test --locked --color never --no-run --message-format=json` **once per `(features, target)`** in-memory, immediately relays `compiler-message.rendered` and child `stderr` across all paths without dumping `compiler-artifact` JSON, enforces `build-finished.success == true`, binds the selected artifact to `cargo metadata` package ID + canonical manifest + canonical target source path + target kind/name + `profile.test == true` + existing executable path (failing closed on missing, mismatched, incomplete, failed, or duplicate artifacts), and runs that exact native libtest binary directly for all queries on `(features, target)` without path guessing or hand-written filter emulation. Doctests stay on their own separate `cargo test --doc` path.
   - **Conservative Empty-Global-Ignored Cache Reuse**: In `CargoDiscovery::list`, when `ignored == true`, `libtest` is a single non-empty `[A-Za-z0-9_:]+` filter atom, and an earlier successful global `--ignored` query for the exact same `(features, target)` key `(features, target, [], true)` already cached an empty set, the filtered `--ignored` query reuses that empty set without spawning another native listing. Positive queries (`ignored == false`), absent/failed/non-empty global sets, changed features or targets, and all other argument shapes (`--exact`, `--skip`, multiple filters, options, whitespace, empty strings, punctuation/quotes, NUL) continue through the unchanged native path without speculative prefetch or test-name matching.
3. **Honest Cost Tradeoffs**:
   - **Mandatory B1 alignment (`rust-contracts`)**: Consolidating `rust-contracts` on `--all-features` eliminated a duplicate default-feature `--bins` compilation as a Stage 1 `B1` fix and is *not* counted as a `B2` optimization.
   - **Added CI work**: Strict default-feature `--all-targets` Clippy (`quality`), strict `cargo doc --workspace --no-deps --all-features --locked` (`quality`), explicit `cargo test --doc --all-features --locked` (`rust-contracts`), and new deterministic Stage 4/5 regression tests add compile/execution time in their respective lanes.
   - **Benchmark runtime parity note**: Accepted historical B0 vs B1 shared-harness comparisons (`18/18` + `9/9`, `129` pairs across `3` pairs per config on a non-isolated host) retain a `cancel` CPU ratio of `1.0769x` (`+20.0ms` CPU / `+20.5ms` lifecycle) alongside `0.97x–1.01x` across the remaining non-cancel cases; broad CPU parity does not include `cancel`.
   - **Final measurement disposition**: B2 measurements are complete, including the final checker mitigation. See the [final report](issue-560-final-report.md) and [portable samples](issue-560-measurements.json); checker median remains above B1 because proof coverage grew. Hosted current-head FULL verification is reported in the PR after independent review.
