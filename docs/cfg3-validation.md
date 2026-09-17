# CFG3 validation record

Validated on Linux against implementation commit
`b02411e1f0a1bbfade6b1f0e7abd45b1a4d0e461`. The following documentation commit
only adds this record. Toolchain: Rust 1.95.0, Node 24.20.0, pnpm 11.13.1,
uv 0.11.12. Upstream remained
`a105022cf0bbcd0cf7eab501a8f4f881a55b3db9` at the final fetch; no integration
was necessary. The original checkout remained clean and untouched.

## Complete validation

Commands run at repository root unless a directory is shown.

| Directory | Command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | Pass |
| root | `git diff --check` and `git diff origin/main --check` | Pass |
| root | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| root | `cargo build --bins` | Pass |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,669 passed, 6 intentionally ignored |
| root | `cargo test --doc --workspace --all-features` | 9 passed |
| test-support/fake-provider | `uv sync --frozen` | Pass |
| test-support/fake-provider | `uv run --frozen pytest` | 51 passed |
| protocol/app-server | `pnpm install --frozen-lockfile` | Pass |
| protocol/app-server | `pnpm check` | Pass; generated protocol/schema/fixture drift absent |
| protocol/app-server | `pnpm typecheck` | Pass |
| tui | `pnpm install --frozen-lockfile` | Pass |
| tui | `pnpm typecheck` | Pass |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 787 passed |
| web-console | `pnpm install --frozen-lockfile` | Pass |
| web-console | `pnpm typecheck` | Pass |
| web-console | `pnpm test` | 283 passed across 21 files |
| web-console | `pnpm check:provenance` | Pass; 74 source records, 100 production package notices |
| web-console | `pnpm build` | Pass; existing bundle-size advisory |
| web-console | `pnpm test:e2e` | 19 passed against real App Server/Product Host and local provider emulator |
| dev | `pnpm install --frozen-lockfile` | Pass |
| dev | `pnpm typecheck` | Pass |
| dev | `pnpm test` | 30 passed |
| root | `cargo run --example generate_schemas` and `git diff --exit-code -- schemas` | Pass; structural schema drift absent |

The six ignored Rust cases are five opt-in live paid/network provider smoke tests
and one fixture-corpus generator that writes committed test documents. They are
not correctness skips. No live paid provider was used. The full Rust totals are
3,060 library tests, 27 CFG3 catalog tests, 5 managed-output tests, 23 conformance,
25 contracts, 128 durable, 52 process, 166 provider, 53 subagent and 130 Tool tests.

## Independent focused checks

| Command | Result |
| --- | --- |
| `cargo test --lib local_runtime::authoring --all-features` | 16 passed |
| `cargo test --lib cfg332 --all-features` | 14 passed |
| `cargo test --lib cfg3_identity --all-features` | 3 passed |
| `cargo test --test cfg3_catalog --test cfg3_managed_output --all-features` | 27 + 5 passed |
| `cargo test --lib child_source_admission_begins_python_preparation_using_the_frozen_package_capture --all-features` | 1 passed |

The example was also checked using `target/debug/rustx config check` with both
`--config` and `--workspace` bound to `examples/local-runtime`. Native analysis
reported Valid with deliberately unresolved placeholder Provider credentials;
it performed no MCP connection or Python preparation.

See the [owner and deterministic invariant-to-test map](../CFG3-WORKLOG.md),
[complete configuration/overlay reference](configuration.md) and
[real-browser screenshots](web-settings.md#browser-evidence).

## Platform scope

These are local Linux results. The existing macOS CI lane retains all relevant
filesystem/process/SQLite boundary classes and now also selects `cfg3_catalog`
and `cfg3_managed_output`; local execution does not claim a macOS result.
No semantic race assertion depends on sleeps. Tests synchronize at native
owner gates, watches/channels, counter observations or injected identity paths.

Full Workflow program editing, Skill/Python source editing, dynamic third-party
Plugins, distributed configuration and migration are documented non-goals,
not deferred implementation pieces.
