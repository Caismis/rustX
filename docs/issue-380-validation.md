# Issue #380 repair validation

This repair continues reviewed HEAD `0bbc1716694c255f965b1d1da8cdf1b2c1cb3cf6`
in the existing `rustX-issue-380` worktree and PR #382. `origin/main` remains
`058ca5280fbba2ccc9a6ed121f464de5f110389c`, already an ancestor; no integration
was necessary. No new PR or worktree was created.

The reviewed HEAD's green CI did not cover healthy-registration no-ops or complete
unit provenance. This revision adds those native regressions and audits source and
runtime composition together. No client suppression of Preparing was added.

The [implementation report](issue-380-implementation.md) contains the revised
ownership model and exact T01–T16 mapping. No wire structures changed during this
repair; the existing generated App Server v14 / RuntimeClient v44 remain current.

## Commands and results

Commands were executed in this worktree on Linux. Directories are relative to it.

| Directory | Exact command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | PASS |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| root | `git diff --check` | PASS |
| root | `cargo build --bins` | PASS |
| root | `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::configuration -- --nocapture` | PASS; 23 deterministic configuration tests |
| root | `cargo test --lib --all-features local_runtime::session_runtime_manager::tests:: -- --nocapture` | PASS; 116 manager, protocol and transport tests |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | PASS; 2,975 tests, one intentional fixture-generator ignore |
| root | `cargo test --test contracts --test provider --all-features` | PASS; 27 contracts and 166 provider tests; five opt-in live-provider tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | PASS; 223 tests |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | PASS; 407 tests |
| protocol/app-server | `pnpm check` | PASS; repository generator executed, no generated drift |
| protocol/app-server | `pnpm typecheck` | PASS |
| tui | `pnpm typecheck` | PASS |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | PASS; 853 tests, no skips |
| web-console | `pnpm typecheck` | PASS |
| web-console | `pnpm test` | PASS; 558 tests in 37 files |
| web-console | `pnpm check:provenance` | PASS; 116 source records and production dependency boundary |
| web-console | `pnpm build` | PASS |
| web-console | `CONTAINER_ENGINE=podman pnpm test:e2e` | PASS; final complete run passed 51 tests |
| dev | `pnpm typecheck` | PASS |
| dev | `pnpm test` | PASS; 37 tests |
| test-support/fake-provider | `uv sync --frozen` | PASS |
| test-support/fake-provider | `uv run --frozen pytest` | PASS; 51 tests |

## Deterministic repair evidence

- T03/T06/T09/T15/T16: healthy policy/context publication followed by new Session
  creation preserves complete Workspace approval/timeout/deadline/capacity metadata.
  No application, deferred flag or candidate exists; natural and repeated load have
  zero configuration preparations and preserve existing resource pointers.
- T06/T09/T16: a pending process restart alone creates no Session application or
  natural-load preparation.
- T03/T05: mixed C1+I2 carries Workspace Instructions provenance in source and runtime,
  retains C1 capability identity and the earlier diagnostic source manifest, and
  registers unresolved C2 work. Retry makes C2+I2 available without rewriting S2.

- T09/T15: the preparation-window regression from the earlier registration repair
  remains intact and passes: S2 initially adopts N, naturally loads N after N+1 is Ready,
  obtains a concrete N+1 candidate, then explicitly adopts with its exact identity
  and expected binding. No second Save/reconcile, model request or history change.
- T03/T05/T13/T15: capability failure after resource construction retains C1
  registry and real Skill guidance while Instructions I2 becomes Ready. S1 adopts
  only Instructions. New S2 receives C1+I2, including C1 Tool definitions and Skill
  packages. Retry makes C2+I2 available to S3 while S1/S2 retain C1. The changed Tool
  selection makes this a context-changing candidate rather than a permitted
  cache-preserving policy update.
- T05/T09: two real Workflow Agent tests Save a workflow-only reviewer's profile
  instructions or model. A provider gate holds an admitted Attempt; it and its
  child use A/M1 after Save. A later Workflow execution sends B/M2 through the real
  child process and model adapter. Preparation counters and generation identities
  prove rebuild; provider equality detects the model change. An unrelated Agent
  addition remains a no-op, and an escaping selected-profile symlink is rejected
  by the same physical-authority check.
- T12/T16: three promise-controlled Settings tests cover source projection before
  acknowledgement, after acknowledgement and after the next edit. Later native
  notifications preserve the new draft and its CAS review boundary.

Browser validation uses the repository's digest-pinned Playwright container and
supported Podman override, with the actual App Server, Product Host and mandatory
provider emulator. Browser plugin not available; the repository Playwright workflow
was explicitly requested. The flow under test is Settings Save, native publication,
subsequent draft editing and explicit Session adoption, plus the full shared-server
acceptance suite at the checked-in desktop and narrow viewports. No screenshot
references, thresholds or browser dependencies were changed.

The exact final-HEAD GitHub run and every job result are recorded in PR #382 after
completion; local validation is not used as a substitute for CI.

## Environment limits

macOS coverage is supplied by GitHub CI, not this Linux host. Five opt-in live
provider tests require external credentials and remain repository-ignored; the
provider emulator is mandatory and passed. The document-corpus generator remains
intentionally ignored. Web build emits its existing large-chunk advisory.
