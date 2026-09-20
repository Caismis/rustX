# Issue #380 repair validation

This repair started at reviewed HEAD `af4f93c37c104aec351289b1cab4bedcf6a2ab20`
in the existing `rustX-issue-380` worktree. `origin/main` remains
`058ca5280fbba2ccc9a6ed121f464de5f110389c`, already an ancestor; no integration
was necessary. No other worktree was changed.

The previous reviewed HEAD failed GitHub Clippy (`unneeded-wildcard-pattern` in
`tests/scripted/app_server/protocol.rs`). Its old all-green validation claim is
superseded by this report. The redundant pattern was removed without a lint allow.

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
| root | `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::configuration -- --nocapture` | PASS; 18 deterministic configuration tests |
| root | `cargo test --lib --all-features local_runtime::session_runtime_manager::tests:: -- --nocapture` | PASS; 111 manager, protocol and transport tests |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | PASS; 2,969 tests, one intentional fixture-generator ignore |
| root | `cargo test --test contracts --test provider --all-features` | PASS; 27 contracts and 166 provider tests; five opt-in live-provider tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | PASS; 223 tests |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | PASS; 407 tests |
| protocol/app-server | `pnpm check` | PASS; repository generator executed, no generated drift |
| protocol/app-server | `pnpm typecheck` | PASS |
| tui | `pnpm typecheck` | PASS |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | PASS; 853 tests, no skips |
| web-console | `pnpm typecheck` | PASS |
| web-console | `pnpm test` | PASS; 555 tests in 37 files |
| web-console | `pnpm check:provenance` | PASS; 116 source records and production dependency boundary |
| web-console | `pnpm build` | PASS |
| web-console | `CONTAINER_ENGINE=podman pnpm test:e2e` | PASS; final complete run passed 51 tests |
| dev | `pnpm typecheck` | PASS |
| dev | `pnpm test` | PASS; 37 tests |
| test-support/fake-provider | `uv sync --frozen` | PASS |
| test-support/fake-provider | `uv run --frozen pytest` | PASS; 51 tests |

The first full Rust contract run exposed a protocol fixture that started an
Attempt before its initial configuration application had settled. The fixture now
waits for native readiness and adopts the concrete candidate if required. It then
checks newly created Sessions only after the next source is available. The full
contract suite passed after that correction.

Browser validation uses the repository's digest-pinned Playwright container and
supported Podman override, with the actual App Server, Product Host and mandatory
provider emulator. Browser plugin not available; the repository Playwright workflow
was explicitly requested. The tested flow is Settings Save / reconcile / explicit
Session adoption, plus reconnect and the full shared-server acceptance suite, at
the checked-in desktop and narrow viewports. Two initial complete runs each had one
single-pixel screenshot mismatch in unchanged presentation fixtures (Agent error,
then Settings inventory). Image comparison found only small antialiasing deltas;
the Agent reference passed unchanged on the next run, and the final complete run passed all 51 tests. No screenshot references,
thresholds, client code or browser dependencies were changed for this repair.

## Environment limits

macOS coverage is supplied by GitHub CI, not this Linux host. Five opt-in live
provider tests require external credentials and remain repository-ignored; the
provider emulator is mandatory and passed. The document-corpus generator remains
intentionally ignored. Web build emits its existing large-chunk advisory.
