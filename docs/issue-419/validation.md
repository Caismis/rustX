# Validation and delivery record (#419)

This records the initial implementation. The subsequent PR #422 review repair
and its final validation are recorded in [repair-422.md](repair-422.md).

## Environment and scope

Base: `6ff49deb4c0855e64f0acaed85ebea2b6a277103` from fetched `origin/main`.
Implementation: `issue-419-session-startup-ownership` in
`/home/caismis/Documents/codes/rustX-issue-419`.
Primary `rustX` remained on `main` at the base, with only its original untracked
`.playwright-mcp/` directory; it was not switched, reset, cleaned, stashed or edited.
A fresh end-of-work fetch observed no main movement. PR #416 remained unmerged;
this implementation does not stack on it. Issue #419 had no new comments.

Read repository instructions, `.github/workflows/ci.yml`, package scripts, pinned
browser instructions and provenance requirements before validation. Node
24.21.0, pnpm 11.13.1, rustc 1.95.0; frozen lockfile installs for protocol,
TUI, Web and dev; `uv sync --frozen` for the existing fake provider. No dependency
upgrade. The browser uses the repository's immutable Playwright 1.63.0 image,
including its font/rasterization contract. Podman supplies the container engine.
Unrelated worktree development servers occupied the default browser ports;
validation temporarily substituted 15173/15174 (baseline 25173/25174), then
restored every port-only edit. No screenshot reference was updated.

## Commands and results

Commands below ran from the implementation root unless a subdirectory is shown.
`CARGO_BUILD_JOBS=2` limits compiler memory use; Rust test threads were limited to
four after concurrent all-target linking exhausted this host's memory/swap.
No assertions, deadlines, settlement requirements or provider-isolation tests
were weakened.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `CARGO_BUILD_JOBS=2 cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins` | Pass; native real-process tests and browser use these binaries |
| `CARGO_BUILD_JOBS=2 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features -- --test-threads=4` | Pass: 3,945 tests, eight explicit ignores across 18 binaries |
| `(cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck)` | Pass; generated outputs committed, regenerated, and checked with no drift; check explicitly names v24 artifacts |
| `(cd tui && pnpm typecheck && RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test)` | Pass: 854 tests, including real stdio/WebSocket transport/startup |
| `(cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance)` | Pass: 1,060 tests / 58 files; 143 provenance records and 131 production package notices |
| `(cd dev && pnpm typecheck && pnpm test)` | Pass: 37 tests |
| `(cd test-support/fake-provider && uv sync --frozen && uv run --frozen pytest)` | Pass: 51 tests |
| `CARGO_BUILD_JOBS=1 cargo test --lib --all-features issue419_measure_native_cold_load -- --ignored --nocapture` | Pass on base and implementation; three samples each with separate build targets |
| `CONTAINER_ENGINE=podman RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir web-console test:e2e` | Pass: 124 browser cases (6.6 minutes), including all real-process and controlled startup cases |
| `(cd web-console && CONTAINER_ENGINE=podman bash scripts/browser-tests.sh startup-measurement.spec.ts startup-ownership.spec.ts)` | Pass: 16 final focused cases; measurement output includes correlated Session/attachment/request IDs |
| Baseline: `(cd web-console && CONTAINER_ENGINE=podman RUSTX_BINARY=/home/caismis/Documents/codes/rustX-419-baseline-target/debug/rustx bash scripts/browser-tests.sh startup-measurement.spec.ts)` | Pass: three correlated samples on an independently rebuilt base binary |
| `git diff --check` | Pass |

The full Rust command includes the repository CI real-boundary lanes: in-crate
boundary suites, `cfg3_catalog` (26), `cfg3_managed_output` (5), `conformance` (22),
`contracts` (28), `durable` (129), `process` (62), `provider` (166), `subagent` (43),
and `tools` (130), plus 3,334 library tests. Physical settlement, cancellation,
terminal ordering and provider isolation tests remain intact.

The eight explicit ignores are two pre-existing opt-in fixture/profile writers,
the new separately executed measurement, and five credential-dependent live
provider tests. Live external providers were not exercised; all provider-dependent
acceptance here uses the existing isolated fake provider. Vite reports its existing
large-chunk advisory; builds pass without a bundle-size policy change.

## Failures resolved and measurement exclusions

- Fixed stale v23 assertions in Rust/TUI, a duplicate type-contract import, a
  Clippy documentation backtick warning, and the obsolete prior-version sentence
  required by the documentation contract.
- Regenerated schema after the final contract-comment edit; no drift check was
  bypassed or redirected to obsolete artifacts.
- Initial unrestricted native concurrency produced one Python capability source
  preparation failure. It passed in isolation and in the complete bounded rerun.
- Concurrent native linking exhausted 30 GiB RAM plus 8 GiB swap and delayed two
  existing browser cases. Task-owned builds were stopped, then completed with
  bounded build/test concurrency; unrelated worktrees/processes were untouched.
  Both browser cases passed on rerun.
- A generated TypeScript rewrite during a browser run triggered Vite's full page
  reload during one Chinese remount case. The trace showed navigation to the
  fixture URL, ending its in-memory client lifetime. Generation and browser
  execution were then separated; this was not treated as a supported hard-reload
  File-persistence guarantee.
- One intermediate full browser run hit an existing Settings confirmation-focus
  assertion; the final complete run passed it without changing its code or checks.
  Two in-progress runs were deliberately stopped before final presentation edits.
- The creation preference test fixture now explicitly supplies an empty preference
  owner, avoiding cross-test preference leakage after restoring native-ACK seeding.
- An initial native baseline using a shared Cargo target reused implementation
  output. Those samples were discarded; the reported baseline was rebuilt in a
  separate target at the base SHA plus only the identical measurement module.

[Ownership/test mapping](ownership.md), [measurements](measurements.md), and the
raw JSON samples identify the supported lifetime and boundaries. Actual DOM
visibility, admission and provider output are separate measurements. No real
provider TTFT, latency target, statistical speedup or exact cross-clock T5 is claimed.

## Visual evidence

Inspected the final English and Chinese attaching, attachment-failed and
admission-failed screenshots. Retained input uses the existing Conversation
composer, pending controls are gated, no native catalog row is fabricated, and
protocol details stay collapsed. The six PNGs are in [evidence/](evidence/).
No Harness/browser reference image or upstream revision changed.
