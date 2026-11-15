# Issue #560 Implementation and Measurement Plan: Adopt Rust 1.100 and Streamline Dependencies, Module Boundaries, Tests, and CI

Status: **Stage 0 Plan Only** (no production code, toolchain pin, dependency manifest, lockfile, or CI changes made yet).

---

## 1. Base Identity, Environment, and Prerequisites

### 1.1 Repository & Toolchain Identity
- **Worktree**: `/home/caismis/Documents/codes/rustX-issue-560` (branch `issue-560-rust-1.100`; primary `/home/caismis/Documents/codes/rustX` remains untouched)
- **Base commit**: `33234e5b19f9074924a5fba1315c23dcf7210c32`
- **Core manifest hashes (SHA-256)**:
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
- **Hardware / OS**: Linux `x86_64`, 16-core `Intel(R) Core(TM) Ultra X7 358H`, `30Gi` RAM + `8.0Gi` swap; `/home` `577G` free / `952G` total; `/tmp` `tmpfs` `12G` free / `16G` total.
- **Native / Python / Node tools verified**:
  - `uv 0.11.12`, system `python3` (`3.15.0`), `.venv` (`CPython 3.12.13` synced in `test-support/fake-provider`)
  - `node v24.21.0`, `pnpm 11.25.0`, `/usr/bin/clang`, `/usr/bin/docker`, `/usr/bin/time`, `/lib64/libclang.so.23.1`
- **Local Node prerequisites (`tui/` and `web-console/`)**:
  - Neither `tui/node_modules` nor `web-console/node_modules` is pre-installed in this fresh worktree. Running `cd tui && node --test "test/**/*.test.ts"` before installing dependencies fails immediately on missing workspace/package setup, and `tsc --noEmit` fails on missing `ajv` / `ajv-formats` modules until `pnpm install --frozen-lockfile` is run in `tui/` and `web-console/`.
- **Memory & raw-log discipline**:
  - Single-job Rust builds are bounded with `CARGO_BUILD_JOBS=4` on this 30 GiB host (`B0` `clippy --all-targets --all-features` already peaked at `9,416,860 KB` ~`8.98 GiB`, and `cargo test --doc` peaked at `8,195,180 KB` ~`7.82 GiB`).
  - All raw timing/compiler logs remain outside git in `/tmp/rustx-560-control/`.

---

## 2. Rust 1.100 Adoption Findings, Blocker Root Cause, and Bounded Cleanup List

### 2.1 Observed Rust 1.100.0 Blocker (`build.rs` + `src/tools/profile_dir.rs`)
Probing `cargo +1.100.0 check --workspace --all-targets --locked` (and `--all-features`, `clippy`, `doc`) in `target/b1-probe` failed immediately during the `rustx` build script (`probe-1.100.0-check-clippy-doc.log`):
- **Observed panic**:
  `assertion 'left == right' failed: refusing to install the Python guard outside <profile>/build/<package>/out: /home/caismis/Documents/codes/rustX-issue-560/target/b1-probe/debug/build/rustx/7d6f99552a8e9c92/out` (`left: Some("rustx")`, `right: Some("build")`).
- **Root cause**:
  - Cargo 1.100.0 enables the new `build-dir` layout by default (`-Z build-dir-new-layout` is active by default), changing `OUT_DIR` from `<profile>/build/<pkg>-<hash>/out` (3 parents above `<profile>`) to `<profile>/build/<pkg>/<hash>/out` (4 parents above `<profile>`).
  - `src/tools/profile_dir.rs` (included by `build.rs` and unit-tested directly) walks up only 3 parents (`out` -> `<hash>` -> `rustx`) and asserts `file_name() == Some("build")`.
- **Stage 1 fix scope (no old-layout fallback per `AGENTS.md`)**:
  - Update `src/tools/profile_dir.rs` and its unit test for the Cargo 1.100.0 4-parent `OUT_DIR` contract (`<profile>/build/<pkg>/<hash>/out`), removing the obsolete 3-parent expectation rather than supporting dual layouts.
  - Note for Stage 2/3 consideration: `build.rs` currently writes `assets/python-guard/{python,python3}` into `<profile>/python-guard` and `<profile>/deps/python-guard` (outside `OUT_DIR`), and `Cargo`'s build-script contract recommends keeping outputs inside `OUT_DIR`. Meanwhile, `src/tools/workspace_python.rs` checks `<current_exe().parent()>/python-guard` first (`provisioned_guard()`, unit-tested at line `3798`) and otherwise uses runtime `fallback_guard`/`ensure_guard` (used by packaged browser artifacts since `web-console/scripts/browser-artifact.ts` does not copy `python-guard`). Stage 1 fixes the 4-parent `<profile>` discovery unblocking 1.100.0 cleanly while keeping the existing executable-adjacent `python-guard` test contract intact; eliminating out-of-`OUT_DIR` writes can be evaluated separately once runtime/packaging parity is verified.

### 2.2 Cargo 1.100 CLI / Build-Cache Behavior Observed
- `cargo +1.100.0 --timings` is a bare flag (passing `--timings=json,html` fails CLI parsing in both `1.98.1` and `1.100.0`). HTML timing reports are written under `<target-dir>/cargo-timings/`.
- `--artifact-dir` remains `(unstable)` in `cargo 1.100.0`, and `-Z build-dir-new-layout` is listed in `cargo +1.100.0 -Z help` while the new `build/<pkg>/<hash>/out` layout is active by default without `-Z`.
- Local HTML docs are absent from the `minimal` rustup toolchain profile (`0` files in `share/doc/rust/html`); API queries use `/tmp/rustx-560-control/cargo-docs.md` / Context7 where available.
- `rustc +1.98.1` and `rustc +1.100.0` both expose `-W unused-crate-dependencies` (default `allow`), useful as an auxiliary check across `--all-targets --all-features` (note: cannot see `build.rs`-only crates or conditional feature combinations on a single invocation).

### 2.3 Toolchain, MSRV, and Documentation Reference Inventory
| Location | Current Baseline Reference | Stage 1 / Stage 5 Action |
| :--- | :--- | :--- |
| `rust-toolchain.toml` | `channel = "1.98.1"` (`profile = "minimal"`, `clippy`, `rustfmt`) | Update to `channel = "1.100.0"` in Stage 1 |
| `Cargo.toml:5-9` | `rust-version = "1.92"` with Issue #48 `xberg` MSRV comment | Update MSRV (`"1.100"` or aligned crate policy) and comment in Stage 1 |
| `.github/workflows/ci.yml` | 11 jobs call `dtolnay/rust-toolchain@stable` with `toolchain: "1.98.1"` (lines `89, 171, 248, 296, 371, 483, 597, 718, 809, 846, 939`) | Update all 11 pins to `"1.100.0"` (prefer centralizing/reading from `rust-toolchain.toml` where practical) in Stage 1 |
| `CONTRIBUTING.md:92` | Documents pinned `1.98.1` toolchain | Update to `1.100.0` in Stage 1 |
| `web-console/scripts/browser-artifact.ts:69` & `web-console/test/browser-verification.test.ts:11, 223` | Doc comment + test metadata references to `1.98.1` | Inspect during Stage 1/5 and align if pinned toolchain version is checked as metadata |

### 2.4 Bounded `B0` Baseline Findings Beyond `build.rs`
1. **`cargo doc --workspace --no-deps --all-features --locked` fails under `RUSTDOCFLAGS="-D warnings"` even on `B0` (`1.98.1`)**:
   - Observed in `/tmp/rustx-560-control/b0-1.98.1-doc.log`: exit status `101`, Elapsed `0:57.28`, Max RSS `4,869,004 KB` (~`4.64 GiB`), failing with **106** broken/private `rustdoc::broken_intra_doc_links` / `rustdoc::private_intra_doc_links` errors in `src/` (plus summary line = 107 `^error` lines). Because CI never runs `cargo doc`, this bitrot accumulated silently.
   - Stage 1/2 cleanup: repair or qualify those broken/private intra-doc links in `src/` so `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked` exits `0`.
2. **All 9 doctests in the workspace pass on `B0` (`cargo test --workspace --doc --all-features --locked`), and all 9 are `compile_fail` doctests**:
   - Located in `src/tools/todo.rs:958` (1), `src/message/types.rs:211, 519` (2), `src/conversation/mod.rs:203` (1), `src/local_runtime/session_controller.rs:129, 132, 138, 144` (4), `src/context/status.rs:392` (1). Execution after build took `0.54s` (`2:50.26` total build+run on `target/b0`, Max RSS `8,195,180 KB`).
   - Currently **no CI job runs `--doc`**, and `examples/check_test_lanes.rs` rejects both `--doc` and `--workspace` inside gated `cargo test` commands.

---

## 3. Candidate Dependency and Feature Cleanup Table (Stage 2)

Cross-referencing all 59 `[dependencies]` and 3 `[dev-dependencies]` in `Cargo.toml` against `rg` call sites, `cargo tree -d`, and crate `[features]` manifests establishes the following **verified** Stage 2 candidates vs retained features:

| Crate | Current `Cargo.toml` Spec | Observed Evidence (`src` / `tests` / `cargo tree`) | Duplicate / Cost Impact | Proposed Stage 2 Action | Validation Scope |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `grep-matcher` | `"0.1.9"` (`[dependencies]`) | **0 direct hits** in `src/`, `tests/`, `examples/`, `build.rs`; only used transitively by `grep-regex` & `grep-searcher` | Unneeded direct edge / lockfile obligation | **Remove** direct dependency from `Cargo.toml` | `grep` native tool tests, `clippy --all-targets --all-features` |
| `nix` (`[dev-dependencies]`) | `{ version = "0.31.3", features = ["signal", "process", "fs", "dir"] }` | Exact strict subset of `[dependencies] nix` (line `89`, which already includes all 4 plus `inotify`, `event`, `user`, `poll`, `socket`, `resource`, `term`, each verified in `src/`) | Duplicate manifest entry | **Remove** redundant `[dev-dependencies] nix`; retain `[dependencies] nix` features | Full `--all-targets --all-features` build & process/FS tests |
| `tower` | `{ version = "0.5", features = ["util"] }` | `src/model/adapter/openai/client.rs` only uses `tower::Service` and `tower::BoxError` (`0` uses of `tower::util` / `ServiceExt`). Note: `async-openai`, `reqwest`, and `axum` currently enable `tower/util` transitively anyway | Removes over-declared direct feature edge (`futures-util`, `pin-project-lite`, etc. from direct manifest) | **Narrow** direct spec to `tower = { version = "0.5", default-features = false }` (verify `BoxError` + `Service` availability) | OpenAI client adapter unit/provider tests & full `check`/`clippy` |
| `mcp-fixture` feature | `mcp-fixture = ["rmcp/server", "rmcp/transport-io", "rmcp/transport-streamable-http-server"]` | Only gated for tests/fixtures (`src/tools/mcp/fixture.rs` + integration/unit MCP tests). Causes CI's default-feature `cargo build --bins` (`rust-contracts` 215s, `tui` 207s) and `--all-features` test builds to maintain **two distinct** `rmcp`/dependency graphs | Forces double compilation of `rustx` library across default vs `--all-features` steps | **Audit & benchmark** keeping `mcp-fixture` behind a dedicated fixture/dev path or aligning CI `--all-features` vs default-feature builds so `rust-contracts`/`tui` do not compile `rustx` twice | `mcp` fixture & boundary tests + `check_test_lanes` |
| `serde_yaml` | `"0.9"` (`[dependencies]`) | `xberg` already compiles **`serde_yaml_ng`** in `Cargo.lock`; `rustx` uses deprecated `serde_yaml` in `workflow_file`, `workflow`, `schemas`, `skills/package`, `launch_tests`, `examples/check_test_lanes.rs` | Duplicate YAML parser crates (`unsafe-libyaml` vs maintained fork / two YAML deserializers) | **Evaluate** consolidating onto `serde_yaml_ng` (already in tree) if 100% deserialization/error compatibility holds on `workflow_file::expansion` budget test | `workflow_file` unit & expansion tests, `runtime_examples`, `check_test_lanes` |
| `jsonwebtoken` | `{ version = "10.4", default-features = false, features = ["aws_lc_rs"] }` | Prod uses `decode`/`Validation`/`DecodingKey` in `src/app_server/transport/auth.rs`; tests use `encode` | No cheaper split (`aws_lc_rs` already shared); **keep as-is** unless `auth.rs` test helpers move | **Keep** (`default-features = false` already tight) | `app_server/transport/auth` & `authentication` tests |
| `minijinja` | `"=2.24.0", features = ["fuel", "json", "unstable_machinery"]` | `env.set_fuel(50_000)` used in `src/runtime/workflow/transform.rs:154`; `tojson` filter tested in `template_limits.rs` & `data_nodes.rs`; `machinery::parse` used at `transform.rs:185` | All 3 declared features are **actively used** | **Keep** (do **not** strip `fuel` or `json`) | N/A |
| `async-openai` | `default-features = true, features = ["chat-completion", "responses", "middleware", "byot"]` | `create_stream_byot`, `HttpRequestFactory`, `chat`, `responses` all verified in `src/model/adapter/openai/` | All 4 features **actively used** (`default` = `rustls`, shared with `reqwest`/`rmcp`) | **Keep** (optionally verify explicit `rustls` with `default-features = false` changes nothing) | N/A |
| `tokio-util` / `schemars` / `uuid` / `getrandom` | `tokio-util` (`rt`, `codec`), `schemars` (`chrono04`), `uuid` (`v7`), `getrandom` (`0.3.4`) | `AbortOnDropHandle` needs `rt`; `LinesCodec`/`FramedWrite`/`Decoder` need `codec`; `CancellationToken` needs `sync`; `DateTime` schemas need `chrono04`; `Uuid::now_v7` needs `v7`; `getrandom::fill` used in 5 `src/` modules (`0.3.4` avoids adding `rand` to root API; `0.2`/`0.4` come from transitive crates `ahash`/`uuid`) | Minor transitive version multiplicity (`base64` 0.22/0.23, `getrandom` 0.2/0.3/0.4, `rand` 0.8/0.9/0.10, `sha2` 0.10/0.11, `toml` 0.8/1.1) driven mainly by `pdf_oxide` / `xberg` / `lopdf` / `rmcp` | **Keep** direct specs unless upstream updates align; do **not** hand-roll RNG/UUID/codec replacements (`AGENTS.md`) | Verify via `cargo update --dry-run` only if explicitly requested in Stage 2; otherwise keep lockfile stable beyond direct cleanups |

*Rule check*: Per `AGENTS.md`, we must not invent ad-hoc replacements for well-maintained crates (`biodivine-lib-bdd`, `httpdate`, `croner`, `globset`, `constant_time_eq`, `pep-508`, `uv-normalize`, `sse-stream`, `unicode-normalization`, `imbl`, `zip`).

---

## 4. Suppression Audit (`#[allow(...)]` Inventory & Disposition)

Regex/AST scan across all Rust files (handling multiline attributes) measured **893** `allow(...)` lint-tokens total:
- **`src/` (713 tokens)**: `clippy::too_many_lines` (**452**), `clippy::too_many_arguments` (**45**), `dead_code` (**31**), `clippy::needless_pass_by_value` (**16**), `clippy::missing_errors_doc` (**13**), `unsafe_code` (**12**), `clippy::large_enum_variant` (**11**), `clippy::struct_excessive_bools` (**11**), `clippy::type_complexity` (**7**), `clippy::cast_precision_loss` (**6**), `clippy::unnecessary_wraps` (**6**), plus minor counts.
- **`tests/` (171 tokens)**: `clippy::too_many_lines` (**107**), `clippy::large_futures` (**34**), `dead_code` (**10**), `clippy::similar_names` (**5**), `clippy::used_underscore_binding` (**4**), `clippy::too_many_arguments` (**2**), `unsafe_code` (**1**), others (**8**).
- **`examples/` + `build.rs` (9 tokens)**: `clippy::cast_precision_loss` (**7**), `clippy::too_many_lines` (**1**), `clippy::unnecessary_wraps` (**1**).

### Stage 2/3 Disposition Plan
1. **`#[allow(dead_code)]` (31 in `src/`, 10 in `tests/`)**:
   - Audit every non-test `#[allow(dead_code)]`: `src/durable/sqlite/projection.rs` (7), `src/model/adapter/anthropic/wire.rs` (5 serde wire structs—either justify as schema-deserialization fields or remove unused fields), `src/runtime/observation.rs` (3), `src/capabilities/coordinator.rs` (2), `src/observation/capability_source.rs` (1), `src/local_runtime/session/lineage.rs` (1), `src/local_runtime/workflow_resources.rs` (1), and `#![allow(dead_code)]` in `src/runtime/permission/bash/{commands,flags}.rs`. Delete genuinely unused items; move test-only helpers behind `#[cfg(test)]`.
2. **Module-wide `#![allow(...)]` blankets**:
   - Narrow `src/runtime/permission/bash/mod.rs` (11 clippy lints at file top), `src/durable/sqlite/auxiliary.rs` (`too_many_lines, type_complexity`), `src/local_runtime/launch_tests.rs` (`needless_pass_by_value, too_many_lines`), and test-crate `#![allow(clippy::too_many_lines)]` in `tests/{boundary/mod,conformance/main,durable/main,process/main,provider/main}.rs` as functions/suites are decomposed.
3. **`clippy::too_many_lines` (452 in `src/`) & `clippy::too_many_arguments` (45 in `src/`)**:
   - Directly targeted by Stage 3 module/helper decomposition in the largest production owners (`workflow.rs`, `background.rs`, `coordinator.rs`, `interaction.rs`, `recovery.rs`, `catalog.rs`, `workspace_python.rs`, `subagent/process.rs`).
4. **`#[allow(unsafe_code)]` (12 in `src/`, 1 in `tests/`)**:
   - All 12 `src/` sites are OS/fd/signal/syscall/seccomp boundaries (`src/runtime/supervised_unit.rs` 5, `src/local_runtime/subagent_child.rs` 2, `src/runtime/process_wait.rs` 2, `src/runtime/interactive_supervisor.rs` 1, `src/runtime/process_supervision.rs` 1, `src/bin/workflow_renderer.rs` 1) plus `tests/boundary/skill_launch.rs:101`. **Retain**, keeping tight item-level scopes and existing `SAFETY` / invariant comments intact.
5. **`#![allow(clippy::large_futures)]` (34 in `tests/`)**:
   - Concentrated in `tests/scripted/app_server/configuration/*.rs`; keep scoped per async test harness unless `Box::pin` in shared driver helpers eliminates the futures bloat naturally.

---

## 5. Bounded Ownership-Based Module Decomposition Plan (Stage 3)

Inspection of the largest `src/` files distinguishes genuinely overloaded multi-concern production owners from large-but-single-concern files or inline test bloat. Compiling `--lib --all-features` on `B0` is already observed to be a multi-minute bottleneck (>4 min in `check_test_lanes` because `rustx` compiles the monolithic crate plus all inline unit tests into one compilation unit).

### 5.1 Selected Split Targets (Bounded to 8 High-Leverage Production Files + Inline Test Separations)

| File (Prod LOC / Total LOC) | Observed Multi-Concern Overload & State/Locks | Proposed Stage 3 Ownership Split | Visibility, State & API Notes |
| :--- | :--- | :--- | :--- |
| `src/runtime/workflow.rs` (`4941 / 7273`) | Despite 16 submodules, root still holds (1) IR/AST types (`WorkflowDefinition`, `WorkflowProgram`), (2) `WorkflowCatalog`, (3) `SchemaMap`/`SchemaProofStep` compiler (~1800 lines), and (4) `WorkflowOutputLatch`/`WorkflowRun`/`WorkflowRuntime` execution (~1200 lines) | Extract `workflow/{catalog,compiler,runtime_execution}.rs` (plus `workflow/tests/root.rs` for lines `4942-7273`) keeping `workflow.rs` as facade re-exporting public types | Preserve `pub`/`pub(crate)` re-exports at `crate::runtime::workflow::*` -> zero external API break |
| `src/tools/background.rs` (`3062 / 6097`) | Combines lifecycle/snapshot DTOs (lines `271-770`), `ConversationBackgroundRegistry` (`Arc<Mutex<BackgroundRegistryState>>`, lines `873-2988`), recovery helpers (`2956`), `test_sync` (`3016`), and 3035 lines of inline tests | Extract `background/{types,registry,recovery,test_sync}.rs` + `background/tests.rs` | Keep `Arc<Mutex<BackgroundRegistryState>>` ownership encapsulated inside `registry.rs`; re-export public surface in `background.rs` |
| `src/capabilities/coordinator.rs` (`2898 / 5015`) | Combines config/types (`CapabilityCoordinatorConfig`, `CapabilityResourceInputs`, `CapabilityObserver`), multi-lock `CoordinatorInner` (`resource_inputs`, `initial_mcp`, `python_store`, wait/pause gates), `PreparedCapabilityCandidate` staging (~2300 lines), `AttemptCapabilityLease`, `test_sync`, and 2117 lines inline tests | Extract `coordinator/{state,staging,lease,test_sync}.rs` + `coordinator/tests.rs` | Keep all locks inside `CoordinatorInner` (`state.rs`) to preserve exact lock ordering |
| `src/runtime/interaction.rs` (`2667 / 4976`) | Combines public request/outcome DTOs (`85-476`), audit/event envelope mapping & `RecordingInteractionAudit` (`477-830`), `InteractionRoute` trait + gates (`831-1196`), `InteractionCoordinator`/`QuestionnaireRequester` (`Mutex<CoordinatorState>` + `Notify` `pending_published`, `1197-2667`), plus 2309 inline test lines | Extract `interaction/{types,audit,route,coordinator}.rs` + `interaction/tests.rs` | Keep `InteractionCoordinator` single-owner lock invariant; re-export existing types from `interaction.rs` |
| `src/runtime/recovery.rs` (`2354 / 3171`) | Combines `RecoveryEvidence` collection/queries (`391-1139`) and `RecoveryPlan` / `RecoveryReconciliation` / `recover()` synthesis (`1140-2353`), plus 817 inline test lines | Extract `recovery/{evidence,plan,reconcile}.rs` + `recovery/tests.rs` | Pure value/planning separation; zero lock changes |
| `src/model/catalog.rs` (`2187 / 3136`) | Combines identity newtypes (`ProviderId`, `ModelId`, `ModelRef`), compatibility enums (`519-700`), credential env resolution (`1188-1251`), TOML document parsing/validation (`1252-1944`), and read-views (`1945-2187`), plus 949 inline test lines | Extract `catalog/{identity,compat,loader,views}.rs` + `catalog/tests.rs` | Re-export `ProviderId`, `ModelCatalog`, etc. at `model::catalog::*`; keep serde attrs intact |
| `src/runtime/subagent/process.rs` (`2132 / 3577`) | Combines pre-spawn/staging & physical root setup (`SubagentSpawnPlan`, `PhysicalChildRuntimeRoot`, `spawn_staged`, lines `95-1350`) with live `ChildDriver` handshake/control loop (`1351-2132`) and 1445 inline test lines | Extract `process/{plan,staging,driver}.rs` + `process/tests.rs` | Preserves `ExecutionObservation` / `ChildControlDispatcher` boundary cleanly |
| `src/tools/workspace_python.rs` (`2067 / 5293`) | Combines global singleton `Owner` (`records`/`flights`/`permits`/`tasks` + test gates, lines `1-1356`), pin/`Requires`/interpreter matching (`1058-1826`), guard provisioning (`1827-2066`), and 3225 inline test lines | Extract `workspace_python/{owner,interpreter,guard}.rs` + `workspace_python/tests.rs` | Keeps `Owner` mutexes/gates cohesive in `owner.rs`; cleanly separates >3.2k test LOC |
| `src/local_runtime/session_controller.rs` (`1575 / 2823`) | Manages `Arc<tokio::sync::Mutex<SessionCatalog>>` + 4 test gates across session read/repair, node tree/clone/fork/copy_lineage, upload/archive, and settings RPCs | Extract `session_controller/{uploads,reads,lineage,settings}.rs` + `session_controller/tests.rs` (preserving the 4 `compile_fail` doctests on `SessionController`) | Preserve `preparation` / `catalog` lock ordering and doctest targets |

### 5.2 Explicitly Deferred / Non-Split Large Files (Bounded Discipline)
- `src/local_runtime/dispatcher.rs` (`889 prod / 1789 total`, `mod tests` starts at `890`): cohesive single-concern `ChildControlDispatcher`; move `mod tests` to `dispatcher/tests.rs` only.
- `src/local_runtime/config.rs` (`1083 prod / 1768 total`): cohesive schema document structs + loader; leave production unified or extract `config/tests.rs` if helpful.
- `src/runtime/interactive_process.rs` (`953 prod / 2111 total`, `mod interactive_tests` starts at `954`): cohesive single state machine; keep production together.
- `src/app_server/protocol.rs` (`2175`): schema-mirrored protocol enums (`Method`, `MethodResult`, `NotificationMethod`) coupled to schema generation; leave intact.

---

## 6. Test Disposition, Determinism, Preparation, and CI/Doctest Ownership (Stage 4 & Stage 5)

### 6.1 Current CI Baseline Costs (`run_id: 43659074269`, `33234e5...`)
Captured from `/tmp/rustx-560-control/b0-ci-snapshot.json` (in-progress run; completed jobs only cited below):
- **Critical-path Rust jobs**:
  - `rust-contracts` (`Deterministic contracts (Linux)`): **`2378s` (`39m 38s`)** total — `215s` default-feature `cargo build --bins`, **`1907s` (`31m 47s`)** running `--lib --bins --examples --all-features`, `139s` pure contract targets, `25s` `check_test_lanes`.
  - `rust-boundaries` (`Boundary suites (Linux)`): **`989s` (`16m 29s`)** — `418s` external boundary targets, `474s` in-crate boundary suites (`cargo test --lib --all-features -- boundary_suites::`).
  - `quality` (`Format and lint`): **`216s` (`3m 36s`)** (`fmt` `9s`, `clippy` `161s`).
  - `browser-prepare`: **`330s`**; completed shards `browser-shard` `1/3` = **`1841s`**, `3/3` = **`1779s`**; `web-client` = **`547s`**; `tui` = **`471s`** (`207s` `cargo build --bins` + `180s` tests); `app-server-protocol` = **`263s`** (`179s` schema/TS drift); `desktop-macos` = **`975s`** (`727s` compiling Rust on macOS).
- **Local `B0` timings observed so far (`1.98.1`, `CARGO_BUILD_JOBS=4`, `/usr/bin/time -v`)**:
  - `cargo fmt --all -- --check`: exit `0`
  - `git diff --check`: exit `0`
  - `python3 -m unittest discover -s benchmarks/runtime -p 'test_*.py' -v`: exit `0` (all passed)
  - `python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v`: `Ran 66 tests in 0.118s`, `OK`
  - `(cd test-support/fake-provider && uv sync --frozen && uv run --frozen pytest)`: `55 passed in 0.18s`
  - `cargo clippy --workspace --all-targets --all-features --locked --target-dir target/b0 -- -D warnings`: exit `0`, Elapsed **`2:30.72`**, Max RSS **`9,416,860 KB`** (~`8.98 GiB`)
  - `cargo test --workspace --doc --all-features --locked --target-dir target/b0`: exit `0` (`9 passed; 0 failed`), Elapsed **`2:50.26`** (test execution `0.54s`), Max RSS **`8,195,180 KB`** (~`7.82 GiB`)
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked --target-dir target/b0`: **FAILED** exit `101`, Elapsed `0:57.28`, Max RSS `4,869,004 KB` (106 intra-doc link errors)
  - `cargo run --profile test --all-features --locked --target-dir target/b0 --example check_test_lanes`: exit `0` (`test lane coverage holds on linux`), Elapsed `7:25.69`, Max RSS `16,656,020 KB` (~`15.88 GiB` compiling `--lib` + 9 integration test targets; **observed Linux FULL runnable inventory: `6,248` tests + `9` unassigned doctests**):
    - `rust-contracts`: `--lib` **5,231** runnable (20 ignored) + `--bins` **0** (all 4 binaries have 0 tests) + `--examples` **27** (`check_test_lanes` 24 + `session_list_benchmark` 3; other 5 examples 0) + `contracts` **31** + `provider` **195** (6 ignored)
    - `rust-boundaries`: in-crate `boundary_suites::` **299** (2 ignored) + `cfg3_catalog` **30** + `cfg3_managed_output` **2** + `conformance` **41** (3 ignored) + `durable` **147** + `process` **69** + `subagent` **45** + `tools` **131** (1 ignored)
  - `cargo test --all-features --locked --test contracts`: exit `0` (`31 passed; 0 failed` in `1.49s`, including `documentation::current_documentation_rejects_obsolete_cfg2_contracts` after adding this plan)

### 6.2 Test Disposition Map (Contract / Unit vs Integration vs Platform / Browser)
Per `tests/README.md` and `.github/workflows/ci.yml`:
1. **Inline/in-crate tests (`src/`, e.g. `runtime_client/{host,projection,trace}/tests.rs` >20k LOC, `local_runtime/{launch_tests,configuration/repair_tests}.rs` >8.4k LOC, `background.rs`/`coordinator.rs`/`interaction.rs`/`workflow.rs`/`workspace_python.rs`)**:
   - **Keep boundary-free pure logic** inside `--lib`, but move oversized inline `mod tests` blocks (>500 LOC) to sibling `tests.rs` submodules (`mod tests;` with `#[cfg(test)]`) for readability and reviewability.
   - **Audit boundary-owning tests in `src/`** (especially `local_runtime/launch_tests.rs`, `tools/native/bash/tests.rs`, `runtime_client/*/tests.rs` that spawn binaries and force `cargo build --bins` prior to `--lib`) to separate pure state-machine unit tests from process-spawning boundary tests where `tests/process/` or `tests/boundary/` already owns real-process proofs, pruning purely redundant subprocess startups while preserving every behavioral assertion.
2. **Integration targets (`tests/{contracts,conformance,durable,process,provider,subagent,tools,cfg3_catalog,cfg3_managed_output}`)**:
   - Keep external integration targets segregated by boundary (`process`, `provider`, `subagent`, `tools`, `durable`, `conformance`, `contracts`).
   - Within the largest multi-thousand-line integration files (`tests/boundary/subagent/conformance.rs` `6515`, `tests/scripted/context/runtime_integration.rs` `4670`, `tests/durable/publication.rs` `4381`, `tests/scripted/extensions/mod.rs` `4221`), trim duplicate fixture-setup boilerplate into shared local builders in `tests/support/` without collapsing distinct semantic cases or bypassing process/provider boundaries.
3. **Sleep -> Determinism & Bounded Poll Eliminations (`sleep(` audit: 20 sites in `tests/`, 64 in `src/`)**:
   - **High-priority bug/waste**: `tests/tools/bash.rs:405-414` runs `for _ in 0..200 { if marker.exists() { break; } sleep(10ms); } assert!(!marker.exists());` — **an unconditional `2.0s` wall-clock sleep on every passing run** proving a negative! Once the shell and process group are reaped/awaited, use process-exit / pipe-EOF confirmation instead of a 2-second timer loop (similarly review `src/tools/native/bash/tests.rs` `28` `sleep(10ms)` loops and `tests/provider/transport.rs:572, 634, 696` `5ms` polling loops where `tokio::sync::Notify` / channel barriers cleanly unblock deterministically).
   - **Justified OS waits / keep bounded**: `tests/common/mod.rs:301, 314` (intentional chunk/header delay fixture), `tests/boundary/mcp_recovery.rs:737, 763` (cross-process journal polling with deadline guard, can optionally switch to FIFO/notify if simple), `tests/tools/bash.rs:1079, 1096, 1237` (OS `kill(..., 0)` ESRCH polling on external child PIDs).
4. **Desktop / TUI / Web / Browser / Protocol (`tui/` 75 test files, `web-console/` 339 test files)**:
   - Keep full Playwright shard coverage (`3/3`), `web-client`, `tui`, `desktop-macos`, and `app-server-protocol` obligations intact; no behavioral skips.

### 6.3 Explicit CI & Doctest Ownership Plan (Stage 5)
1. **Doctest Ownership**:
   - Assign `cargo test --doc --all-features --locked` (covering all 9 `compile_fail` doctests + any future doctests) explicitly to `rust-contracts` (Linux) AND update `examples/check_test_lanes.rs` (`parse_command` and `REQUIREMENTS`) to recognize and require `--doc` coverage (and accept or normalize `--workspace` where applicable) so `check_test_lanes` cannot pass if doctests are dropped.
   - Add `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked` to `quality` once all `B0` intra-doc link errors are resolved in Stage 1/2.
2. **Eliminate Double `rustx` Graph Builds in CI**:
   - Today `rust-contracts` (`215s`), `tui` (`207s`), `browser-prepare` (`225s`), and `app-server-protocol` (`179s`) build default-feature binaries/examples **without** `mcp-fixture`, while `rust-contracts` / `rust-boundaries` immediately follow with `--all-features` (`mcp-fixture` activates 3 extra `rmcp` server/transport features). Moving `mcp-fixture` cleanly or normalizing binary prep flags avoids rebuilding `rustx` under two different feature sets in the same job (`rust-contracts` saves ~`200s` per run!).
3. **Cache Key & Runner-Minute Efficiency**:
   - Keep `Swatinem/rust-cache@v2` `cache-on-failure: false`, split/prune oversized `target/` artifacts (verify `cache-targets` settings), keep `CARGO_BUILD_JOBS=4` bounded appropriately on macOS (`CARGO_BUILD_JOBS: "1"` only if OOM requires it—measure on B2) and preserve `cargo-timings` artifact retention (`7d`, small HTML files).

---

## 7. Grounded Measurement Procedure & Optimization Targets (B0 / B1 / B2)

### 7.1 Comparable Commands Across Checkpoints
All local timings run on this idle Linux host with `/usr/bin/time -v`, `CARGO_BUILD_JOBS=4`, `--locked`, and comparable `--target-dir` directories (`target/b0` on `1.98.1`, `target/b1` on `1.100.0` immediately post-Stage-1, `target/b2` on `1.100.0` post-Stage-5), logged under `/tmp/rustx-560-control/`:

1. **Cold native check & build (`--timings`)**:
   - `rm -rf target/<chk>-cold && /usr/bin/time -v env CARGO_BUILD_JOBS=4 cargo check --workspace --all-targets --all-features --locked --target-dir target/<chk>-cold --timings`
   - `/usr/bin/time -v env CARGO_BUILD_JOBS=4 cargo build --bins --all-features --locked --target-dir target/<chk>-cold --timings`
2. **Dependency-hit rebuild** (modify `src/tools/todo.rs` whitespace/comment or `touch src/lib.rs`, rebuild `--lib --tests`):
   - `/usr/bin/time -v env CARGO_BUILD_JOBS=4 cargo check --workspace --all-targets --all-features --locked --target-dir target/<chk>-cold`
3. **Incremental leaf-module edit rebuild** (repeated 3x: append harmless comment change in a leaf source file, measure `check` + `test --no-run` + single unit test execution):
   - `/usr/bin/time -v env CARGO_BUILD_JOBS=4 cargo test --all-features --locked --target-dir target/<chk> --lib -- tools::todo::`
4. **Lint, Doc, and Doctest**:
   - `cargo clippy --workspace --all-targets --all-features --locked --target-dir target/<chk> -- -D warnings`
   - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked --target-dir target/<chk>`
   - `cargo test --workspace --doc --all-features --locked --target-dir target/<chk>`
5. **Test prep (`--no-run`) vs Execution**:
   - Separate `--no-run` compile/link time from binary execution time for `--test contracts`, `--test process`, `--test provider`, `--test durable`, `--test conformance`, `--lib` (repeated 3x where timing noise exceeds 5%).
6. **Runtime workload (`benchmarks/runtime/README.md`, Issue 481 harness — NOT deprecated `run.py`)**:
   - Build preserved release conformance binaries via `python3 benchmarks/runtime/build-revision.py --repo ... --target ... --output ...` for `B0`, `B1`, `B2` (verifying `fresh:false` and SHA-256 identity).
   - Run paired interleaved comparisons with `python3 benchmarks/runtime/compare-revisions.py --before ... --after ... --pairs 3` and append benchmarks `python3 benchmarks/runtime/compare-append.py` (normal + `--cold`) + `python3 benchmarks/runtime/summarize-comparisons.py`.
   - Zero real/paid model API calls (uses local Python HTTP/SSE emulator + Durable SQLite).

### 7.2 Observed `B0` vs Grounded Stage Targets (*Targets Below Are Hypotheses to Verify on B1/B2*)
| Metric | Observed `B0` (`1.98.1`) | Expected `B1` (`1.100.0` Adopt Only) | Target `B2` (After Stages 2–5) | Mechanism Removing Measured Cost |
| :--- | :--- | :--- | :--- | :--- |
| `clippy --all-targets --all-features` | `2:30.72`, `8.98 GiB` Max RSS (cold `target/b0` run) | Measure `B1` (`1.100.0` / LLVM 23) | Keep or reduce RSS & wall time | Fewer redundant deps (`grep-matcher`, feature alignment), smaller functions/modules |
| `doc --no-deps --all-features` (`-D warnings`) | **FAIL (101)** in `0:57.28`, `4.64 GiB` RSS (106 errors) | Fix blocker or Stage 1+2 clean **0 errors** | **PASS (exit 0)** | Repair 106 private/unresolved intra-doc links |
| `--doc` tests | **PASS (9 passed)** in `2:50.26` (`0.54s` exec), **0 CI coverage** | **PASS** | **PASS** + **gated in CI** | Add `--doc` to `rust-contracts` & `check_test_lanes.rs` |
| `rust-contracts` CI job wall clock | **`2378s`** (`39m 38s`; `215s` bins + `1907s` lib/bins/ex) | Baseline `B1` measurement | **>=15–25% reduction** (~`<=1800s` hypothesized) | Eliminate double `mcp-fixture`/default-feature graph build in `rust-contracts`, remove 2.0s unconditional sleeps & redundant subprocess setups, split monolithic `src/` files & inline test bloat |
| Cold / Incremental leaf edit latency & RSS | To be measured cleanly in Stage 1 (`B0` vs `B1` cold) | Measure `B0` vs `B1` before cleanups | Target lower incremental link/codegen wall time & <=9 GiB RSS | Smaller `workflow`/`background`/`coordinator`/`interaction` modules + cleaner dep graph |
| Runtime HTTP/SSE & append conformance (`481` harness) | To be built via `build-revision.py` for `B0` | Compare `B0` -> `B1` (rustc `1.100` / LLVM 23) | Zero statistically significant throughput/memory regression (`B2` vs `B0`) | Only non-functional refactors + dependency trims |

---

## 8. Proposed Staged Commit Sequence & Verification Gates

- **Stage 0 (this commit)**: `Add Stage 0 plan for Rust 1.100 adoption` — `docs/engineering/issue-560-plan.md` only. Verified by `git diff --check` and `cargo test --test contracts` (`documentation.rs`).
- **Stage 1 (`Adopt Rust 1.100.0 toolchain and fix Cargo 1.100 build-dir layout`)**:
  - Update `rust-toolchain.toml`, `Cargo.toml` MSRV, `.github/workflows/ci.yml` toolchain pins, `CONTRIBUTING.md`, `src/tools/profile_dir.rs` (4-parent Cargo 1.100 layout), and fix intra-doc link errors blocking `RUSTDOCFLAGS=-D warnings cargo doc` (or keep any large doc sweeps cleanly split). Record `B0` vs `B1` timings.
- **Stage 2 (`Remove unused dependencies, narrow features, and tighten suppressions`)**:
  - Clean `Cargo.toml` (`grep-matcher`, duplicate `nix` dev-dep, `tower`, evaluate `serde_yaml_ng` / `mcp-fixture`), `Cargo.lock` sync, remove dead `#[allow(dead_code)]` items and redundant `#![allow(...)]` blankets.
- **Stage 3 (`Decompose overloaded runtime, capability, tool, and model modules`)**:
  - Ownership-based module splits across the 8–9 bounded targets in Section 5 (`workflow`, `background`, `coordinator`, `interaction`, `recovery`, `model/catalog`, `subagent/process`, `workspace_python`, `session_controller`), extracting inline `mod tests` (>500 LOC) to sibling `tests.rs` files.
- **Stage 4 (`Consolidate test helpers and replace fixed polling sleeps with barriers`)**:
  - Eliminate unconditional polling waits (`tests/tools/bash.rs:405-414` etc.), audit subprocess-heavy unit tests (`src/tools/native/bash/tests.rs`, `src/local_runtime/launch_tests.rs`), share test fixture builders cleanly.
- **Stage 5 (`Align CI workflows, doctest coverage, and record B0/B1/B2 verification`)**:
  - Wire `cargo test --doc --all-features --locked` and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked` into CI + `examples/check_test_lanes.rs`, unify bin/test feature builds to cut double compilation, install/verify `tui` & `web-console` suites, run full `B0`/`B1`/`B2` compile + runtime benchmark comparisons (`benchmarks/runtime/compare-revisions.py`), update plan/evidence doc with final observed numbers.
