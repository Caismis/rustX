# Issue #380 validation

Validation ran in the isolated `rustX-issue-380` worktree on Linux. The branch
started at `1c5a493a87c15b30874cc39bd4f676525db9a87e` and was rebased onto
`058ca5280fbba2ccc9a6ed121f464de5f110389c`. The integrated Web Agent Status/Todo
changes were retained and migrated to the new protocol. No other worktree was changed.

The [implementation report](issue-380-implementation.md) maps T01–T16 to exact
tests and documents their synchronization and linearization points.

## Commands and results

Commands below completed successfully; directories are relative to the worktree.

| Directory | Exact command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | PASS |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| root | `git diff --check` | PASS |
| root | `cargo build --bins` | PASS; real process and supervisor binaries built |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | PASS; 2,965 tests, one fixture-regeneration test intentionally ignored |
| root | `cargo test --test contracts --test provider --all-features` | PASS; 27 contracts and 166 provider tests; five opt-in live-provider tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | PASS; 223 tests |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | PASS; 407 tests |
| root | `cargo test --lib --all-features local_runtime::` | PASS; 452 tests |
| protocol/app-server | `pnpm install --frozen-lockfile` | PASS |
| protocol/app-server | `pnpm generate` | PASS; normal v14 generation |
| protocol/app-server | `pnpm check` | PASS; generated artifacts have no drift |
| protocol/app-server | `pnpm typecheck` | PASS |
| tui | `pnpm install --frozen-lockfile` | PASS |
| tui | `pnpm typecheck` | PASS |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | PASS; 853 tests, no skips |
| web-console | `pnpm install --frozen-lockfile` | PASS |
| web-console | `pnpm typecheck` | PASS |
| web-console | `pnpm test` | PASS; 555 tests in 37 files |
| web-console | `pnpm check:provenance` | PASS; 116 source records, 100 production dependency notices |
| web-console | `pnpm build` | PASS |
| web-console | `CONTAINER_ENGINE=podman pnpm test:e2e` | PASS; 51 tests |
| dev | `pnpm install --frozen-lockfile` | PASS |
| dev | `pnpm typecheck` | PASS |
| dev | `pnpm test` | PASS; 37 tests |
| test-support/fake-provider | `uv sync --frozen` | PASS |
| test-support/fake-provider | `uv run --frozen pytest` | PASS; 51 tests |

Browser tests used the repository-supported Podman override because Docker was
unavailable. The repository's pinned Playwright image, real rustx binary and
provider emulator were used. Screenshot thresholds were not relaxed. Changed
Settings and Agent reference captures were regenerated through the repository
browser script and inspected. Documentation captures come from the passing real
App Server acceptance run.

Initial failures exposed obsolete publication expectations, model capture reading
an unrelated resource directory, and asynchronous test admission/settlement races.
Those were corrected at their owning layers and rerun. A healthy unchanged rescan
now retains the available candidate without rebuilding it. Failed immutable input
capture reports an absent revision rather than inventing an input identity. Final
request-shape checks also cover context-window changes, which conservatively
require adoption even if the configuration-only wire probe is unchanged.

The final Rust quality and contract lanes were repeated after that comparison
change. Client, process/resource and browser lanes had already passed after the
main integration; no client or resource implementation changed afterward.

## Environment limits

macOS CI was not run on this Linux host. Five repository-ignored live-provider
smoke tests require external credentials/network access; local adapter tests and
the mandatory provider emulator passed. The ignored document-corpus generator is
an intentional maintenance action, not an acceptance test. Web build emits its
existing large-chunk advisory; the build succeeds.
