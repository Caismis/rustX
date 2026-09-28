# PR #424 contract repair and native CI measurement

## State and delivery scope

Initial clean dedicated worktree: `/home/caismis/Documents/codes/rustX-issue-421`,
branch `issue-421-trajectory-semantic-ledger`, HEAD
`50208f34abd9746e7ee1472920b6b46e25098afd`. Fetched main and merge base were both
`f4c044e9f9aa519254b3f7279de847a4df0ae992`; main was already integrated.
The primary worktree and its untracked `.playwright-mcp/` were not changed.

The Ledger uses the exact native record membership already established by
`projectTrajectory`. Message groups accept Attempt-owned records with absent
Step identity, including JSON `null`. No synthetic Step, positional ownership,
renderer fix, or separate keyboard/search workaround is introduced. Native group
order, compact folds, and 10/20/30px seats remain unchanged. Explicit-null tests
cover native inbound adoption serialization, expanded/folded membership, multiple
inputs, partial windows, overlapping prepends, search/fold restoration, logical
focus and exact Inspector ownership. Browsing and structural inspection do no
heavy reads; selecting the User requests only that exact record's detail.

The only current vocabulary is Runtime Client **53** / App Server **28**.
Earlier generations are rejected at admission. Tool arguments remain bounded to
512 UTF-8 bytes; unavailable proposals remain null; one exact native proposal
resolution supplies name and arguments. Lifecycle refresh does not resolve the
immutable proposal again. Generated v27 files are removed. No compatibility path
or storage migration is introduced. The real uv/FastMCP publication gate and
frozen-source authority from `50208f34` remain intact.

Harness reference remains `477b4f420553e8a52c2fbccc464d7561b239c443`.
Source-inventory hashes are updated for reviewed projection and protocol imports
(19 local hashes changed; exact old/new values are in the commit diff). The Ledger
projection hash changes from
`c3d1868d57d9513a13b85987061139516f171d1ec6e64bc25148fe543be169e1` to
`a6931742e64106d1ae58a27517c7bf3dc1fb5349d1c9f1a878f0bc06e2eeacf3`.

## Audited native macOS evidence — old HEAD only

[Run 36424727417 / job 108935767107](https://github.com/Caismis/rustX/actions/runs/36424727417/job/108935767107)
ran HEAD `50208f34`. The check annotation states: “The job has exceeded the
maximum execution time of 40m0s”. The external step logs “The operation was
canceled.” This is a job-budget cancellation, not a test assertion failure.

| Measurement | Observed evidence |
| --- | --- |
| Runner | macOS 26.6.2 (25G83), image `macos-26-arm64` `20260907.0351.1`, aarch64 |
| Compiler | stable rustc 1.98.1 (`48a229cea`, 2026-09-01) |
| CPU / memory | Not recorded in this job; do not infer capacity from the runner label |
| Cargo configuration | No repository `.cargo` configuration or Cargo profile overrides; `stable` minimal toolchain with Clippy/rustfmt; `CARGO_INCREMENTAL=0` from toolchain action |
| Rust cache | MISS, no bytes restored; cache step 12:53:29–12:53:30 UTC, lookup 12:53:30.586–12:53:30.794 |
| Cache key | `v0-rust-rust-platform-boundaries-Darwin-arm64-2eab217e-b2613dc5` (restore prefix ends at `2eab217e`) |
| Real binaries | `cargo build --bins`: step 8m17s; Cargo reports 8m16s |
| Binary build breakdown | Step start 12:53:35; first `Compiling rustx` 12:58:32.656; finish 13:01:52.020. About 4m58s before rustX, then 3m19s in the rustX build span |
| Lib/bin test step | 31m04s total, successful |
| Test compilation | Cargo reports **20m45s**; first `Compiling rustx` 13:04:00.531 to finish 13:22:38.104: about **18m38s** in the rustX build span; about 2m08s before it |
| Test execution | Library harness **617.40s** (10m17.40s): 2,639 passed, 2 ignored; three bin harnesses each 0 tests / 0.00s |
| External targets | Step ran 41s; compilation began, some linker warnings emitted; no completed compilation or test result. **Not passed** |
| Entire job | 12:53:19–13:33:47: **40m28s**, cancelled |

These are aggregate Cargo spans, not per-unit timings: the old logs cannot split
ordinary library, cfg(test) library, binary, test harness, codegen and linker
costs precisely. The confirmed dominant cost is the rustX test-build span, not
31 minutes of test execution. Repeated dependency compilation is visible in the
second step: 61 dependency package names occur in both phases (including rmcp,
Tokio, serde and their dependents). Package names are not artifact identities;
this count does not establish that all 61 builds were avoidable. The first command
uses default features while tests use `--all-features`; dev dependencies also
change feature unification. Ordinary and cfg(test) libraries cannot share a
single artifact. The external targets necessarily add distinct executable units.

The linker reports an oversized `__eh_frame` compact-unwind table. That is an
observed warning, **not evidence of linker duration or memory pressure**. No OOM,
RSS measurement or swap-pressure evidence is available. Debug information cost
is a hypothesis being evaluated, not a measured native speedup.

## Bounded CI changes and comparison procedure

Only the macOS job uses `CARGO_PROFILE_DEV_DEBUG=1` and
`CARGO_PROFILE_TEST_DEBUG=1`. This changes debug metadata, retaining Cargo's
assertions, overflow checking, optimization, unwind and release defaults. Local
developer defaults and Cargo.toml remain unchanged. Both settings participate in
rust-cache's default `CARGO` environment-prefix hash.

The real `target/debug/` binaries are prebuilt with `--all-features`, matching the
subsequent tests. `mcp-fixture` enables the existing MCP fixture/server code;
`issue-387-profile` enables the existing measurement counters (also enabled under
cfg(test)). No new feature or runtime mode is added. Default-feature real-binary
validation remains in the Linux contract, boundary and TUI lanes and is run
locally. Binary harnesses are retained and remain distinct from real binaries.
Dev-dependency feature unification and cfg(test) can still require separate
builds; this change does not claim to eliminate those necessary units.

The macOS selectors, skips, external target list, provider-emulator requirement,
40-minute job budget and test deadlines are unchanged. `--no-run --timings`
separates compilation; the original Cargo test commands then execute unchanged
selectors. `/usr/bin/time -l` forwards the Cargo status and reports wall time and
peak RSS. There is no tee pipeline or custom test runner. Timestamped Cargo HTML
reports are retained as a seven-day artifact with best-effort `always()` upload;
job cancellation can still prevent final artifact publication. The job summary
records actual CPU/memory, compiler, cache-hit output and profile settings.

Cache lineages remain separate by lane and platform/toolchain/configuration.
Workspace crates remain uncached; PRs only restore; only trusted main pushes
publish. A cold cache remains a supported execution path.

Native before/after comparison is **pending**: this execution environment is
Linux x86_64, not macOS. No native speedup or revised minimum budget is claimed.
On the same native macOS runner/toolchain, prefetch the same locked dependencies
(`cargo fetch --locked`), then use two empty target directories, with
`CARGO_INCREMENTAL=0` for both. Baseline: DEBUG=2 for dev/test and the original
`cargo build --bins`; variant: DEBUG=1 for dev/test and
`cargo build --bins --all-features`. For each, run the exact lane's lib/bin and
external compilation with `--no-run --timings`, followed by the unchanged test
commands, all under `/usr/bin/time -l`. Keep the same CPU, memory, source, lockfile
and dependency download state. Compare the cold totals and per-unit reports,
not warm no-op runs. For source-only comparison, prewarm each configuration's
dependencies, then `cargo clean -p rustx` within each isolated target directory
before measuring; do not clean or mutate a developer's shared target directory.
Record default/all-feature unit differences independently from debug-info gains.
If the complete optimized cold lane still exceeds 40 minutes, report its measured
full duration and limiting phase separately before proposing a budget change.

## Validation of the repaired source

Local validation covers the repaired source in Ledger commit `4176682f` and
protocol commit `174bd983`; the following CI commit adds only workflow and this
validation record. These results do not reuse old-HEAD CI. Local host: Linux
x86_64, rustc 1.95.0 (`59807616e`), Node 24.21.0, pnpm 11.13.1, uv 0.11.12.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed; real default-feature binaries |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | Passed: 4,033 tests, 0 failures, 8 existing ignored; includes all seven external targets |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | Passed: 194 tests, 0 failures |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority --exact` | Passed: 1 exact test, 0 failures |
| `(cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck)` | Passed; generated v28 artifacts have no drift |
| `(cd tui && pnpm typecheck && RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test)` | Passed: 892 tests |
| `(cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance)` | Passed: 1,153 tests / 64 files; provenance 145 sources / 131 packages |
| `(cd web-console && pnpm exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts test/trace-cache.test.ts test/client.test.ts)` | Passed: 191 focused tests |
| `cargo test --lib --all-features -- runtime_client::trace::` | Passed: 78 tests, including native argument summary bounds/ownership/lifecycle and null Step serialization |
| `cargo test --lib --all-features -- app_server::schema::tests::` | Passed: 7 schema, wire round-trip and generated fixture tests |
| `cargo test --lib --all-features -- attachment_request_correlation_and_version_negotiation` | Passed: rejects every earlier Runtime Client generation |
| `cargo test --lib --all-features -- initialize_and_malformed_wire_are_transactional` | Passed: rejects every earlier App Server generation without admission side effects |
| `(cd web-console && CONTAINER_ENGINE=podman pnpm test:e2e)` | Passed: all 142 tests in the repository-pinned Playwright container, 8.9 minutes |
| `git diff --check` | Passed |

Workflow YAML was parsed and compared with the reviewed workflow: all non-macOS
jobs are unchanged; macOS execution selectors, provider requirement, cache publish
policy and budget are identical. Compilation/execution are separate Cargo calls,
not a custom runner. Native macOS execution, timing reports, peak-memory results,
and comparable cold before/after measurements are **pending/unavailable locally**.
No Linux result substitutes for that gate. No macOS speedup or minimum larger
budget has been measured. Remote CI for the delivered commits remains a merge gate.

All affected screenshot assertions passed with existing baselines and tolerances.
The desktop English exact-order and narrow Chinese compact-fold images were also
visually reviewed: retained System/main content, Request markers and compact seats
remain intact. No screenshot baseline changed. The exact changed local hashes are
recorded in `web-console/source-inventory.json` in the two contract commits;
upstream source revisions and hashes are unchanged, including the pinned #421
Harness reference. The Vite build emits its existing large-chunk advisory; it
completed successfully.

The original managed-source repair in `50208f34` remains in this branch: an
explicit one-shot pre-publication acknowledgement gates the store-owned preparer,
then a release and owner completion precede availability selection and invocation.
Workspace mutation while held proves the frozen prepared source supplies both.
The timeout is a post-release liveness guard, not synchronization over uv setup.
No sleep, timeout increase, retry, cancellation change or authority fallback is
added by this repair.
