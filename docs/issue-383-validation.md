# Issue #383 implementation validation

Worktree: `/home/caismis/Documents/codes/rustX-issue-383`.
Branch: `issue-383-native-extension-convergence`.
Initial base and re-fetched integration base:
`4ee2248e11077ac9efa29fea5762dc9ffe3f58b9`. No rebase was necessary.
No other worktree was modified.

The [ownership/boundary contract and complete acceptance matrix](native-context-contributions.md)
records exact regression names and synchronization. Agent Status and Goal now
share the outer mechanism; Time/Background/Todo share the internal section
contract. No provider adapter needed extension semantics.

## Local CI-equivalent results

Linux x86_64; Rust/Cargo 1.95.0, Node 24.20.0, pnpm 11.13.1, uv 0.11.12.
The browser lane used the CI-pinned Playwright 1.63.0 Noble image with Podman.
The Browser skill was absent; the frontend-testing-debugging skill selected the
repository Playwright workflow. React best-practices guided the small typed
Trace renderer change; no client-owned domain state was added.

| Command (repository root unless specified) | Final result |
| --- | --- |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --all-targets --all-features` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,992 passed; one existing fixture-regeneration test ignored |
| `cargo test --test contracts --test provider --all-features` | 27 contracts + 166 provider tests passed; five existing opt-in live-provider tests ignored |
| fake-provider: `uv sync --frozen` | Passed |
| fake-provider: `uv run --frozen pytest` | 51 passed |
| `cargo test --lib --all-features -- boundary_suites::` | 223 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 407 passed: 129 durable, 52 process, 43 subagent, 130 tools, 22 conformance, 26 catalog, 5 managed-output |
| protocol/app-server, tui, dev, web-console: `pnpm install --frozen-lockfile` | Passed in all four packages |
| protocol/app-server: `pnpm generate` | Passed; generated v16 artifacts are coherent |
| protocol/app-server: `pnpm check && pnpm typecheck` | Passed, including committed-generation drift check |
| tui: `pnpm typecheck` | Passed |
| tui: `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 853 passed, including actual App Server integration |
| dev: `pnpm typecheck && pnpm test` | Passed; 37 tests |
| web-console: `pnpm typecheck && pnpm test` | Passed; 558 tests |
| web-console: `pnpm check:provenance` | Passed; 116 source records and 100 production-package notices |
| web-console: `pnpm build` | Passed; existing large-bundle advisory is non-fatal |
| web-console: `CONTAINER_ENGINE=podman pnpm test:e2e` | 51 passed; all existing screenshot references unchanged |

Focused contributor, status, assembly, Goal, SQLite, and startup regressions were
also run during implementation. The final full source lane includes all 14
`issue383_` tests. No required test was skipped or reclassified.

Intermediate failures were fixed, not waived: pre-build supervisor availability;
obsolete request-start/Trace fixtures; old protocol literals; the skipped-test
identity schema artifact; typed TS fixtures; provenance source hashes; and a
browser assertion that incorrectly expected a separate Trace history request
when that history already came in the native Session snapshot. Every affected
lane was rerun. Five live-provider/network smoke tests and the corpus-writing
test retain their pre-existing explicit opt-in classification.

## Browser QA

Real App Server + provider emulator, desktop 1440x1000 and narrow 390x844;
the suite also covers 820, 1280, and 1600 widths.

| Check | Evidence |
| --- | --- |
| Correct, nonblank page | Real Session creation/reattachment and canonical Chat/Trajectory content |
| No framework overlay / app errors | Browser assertions on Vite overlay and collected page/console errors |
| Historical contribution interaction | Open actual Trace request → Context tab → typed accepted producer, message identity, sections, and native opportunity anchor |
| Current domain separation | Todo/Goal docks follow live authority; historical Status placement survives reload and later Goal edits |
| Visual proof | Existing desktop/mobile screenshot suite passed; inspected `/tmp/rustx-383-accepted-contributions.png` |
| Configuration/session behavior | Real integrations, recovery, settings, dev launcher, separate Product Hosts, fork/reload and archive scenarios passed |

The Linux environment cannot execute a macOS kernel lane. The unchanged
`Platform-sensitive boundaries (macOS)` GitHub job is the platform authority;
its status is reported on the implementation PR, never represented as a local
macOS pass. No opt-in live-provider credentials were used.
