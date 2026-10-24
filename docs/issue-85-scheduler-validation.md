# Scheduler integration validation

Validated 2026-10-24 on Ubuntu 24.04 / WSL2 ext4, Rust 1.98.1,
Node 24.21.0 and pnpm 11.13.1. The branch integrates origin/main
`00978bbe3` with App Server v56, Runtime Client v75, SQLite schema 80,
Session catalog 18 and child IPC 34. All work ran in the dedicated
`issue-85-scheduler-design` worktree without subagents.

| Check | Result |
| --- | --- |
| Optimized all-feature native library | 4754 passed, 8 explicitly ignored |
| Independent contracts | 31 passed |
| Real CLI/App Server process suite | 65 passed, 0 ignored |
| Final scheduled Workflow preparation fixtures | 9 passed, including static Tool/Agent/Model dependencies nested through Loop/Parallel |
| Scheduler App Server RPC contracts | 2 passed inside the library suite |
| Scheduler WAL-lock and cancellation fixtures | 4 passed; transient-lock regression was red before the bounded wait fix |
| Strict Clippy | all targets / all features, `-D warnings`, passed |
| Rust formatting / diff whitespace | passed |
| Generated protocol, TUI and Web type checks | passed |
| TUI with real native binary and provider emulator | 1194 passed, 1 macOS ACL test skipped |
| Web full suite with real native fixture paths | 3235 passed, 4 macOS tests skipped |
| Web source/dependency provenance | 311 source records and 287 production dependency notices verified |
| Web internationalization / production build | passed |
| Native scale/idle resource gate | schema 80 matrix plus ten-pass, ten-minute idle gate passed |

Native binaries and both supervisors were built with all features in release
mode. Web tests' fixed `target/debug` fixture paths pointed to these same release
binaries; `RUSTX_BINARY` selected the release rustx explicitly. Provider emulator
dependencies were restored from their lockfile with `uv sync --frozen --offline`.
Optional measurement tests remain ignored in ordinary correctness runs and ran
separately for the [performance acceptance](issue-85-scheduler-performance.md).
Browser screenshot acceptance and macOS execution are outside this Linux run.

The native library, contracts and process checks used:

```sh
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --offline --release \
  --lib --test contracts --test process --all-features -- --test-threads=4
cargo clippy --offline --release --all-targets --all-features -j2 -- -D warnings
cargo fmt --all -- --check
```

The delivered product supports native inbound At/interval/Cron scheduling,
trusted exact-node controls and read-only Web/TUI projection. Workflow preparation
is bounded and effect-free, with semantic reconstruction/refusal fixtures.
Durable Workflow start/claim/settlement/recovery and live Human-request model
mutation authority remain unimplemented; this change does not close issue #85.
