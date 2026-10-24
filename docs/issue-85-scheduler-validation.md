# Scheduler integration validation

Validated 2026-10-24 on Ubuntu 24.04 / WSL2 ext4 with Rust 1.98.1,
Node 24.21.0 and pnpm 11.13.1. The branch includes origin/main through
`00978bbe3`. Current formats are App Server 57, Runtime Client 76, SQLite 81,
Session catalog 18 and child IPC 35. Older stores and peers are refused without
migration or compatibility readers. Work used the dedicated
`issue-85-scheduler-design` worktree without coding subagents.

## Verified behavior

Both temporal targets use the native conversation writer: Runtime Message
admission and pinned Scheduled Workflow admission. Accepted-to-Claimed issues one
execution capability; Tool, Model, Agent and Review nodes share the existing
Workflow executor and physical owners. The Scheduled run and interaction scope
contains no synthetic Root Attempt, ToolCall or model turn. Live and historical
interaction audit projections preserve absent Attempt/turn metadata.

Native cold loading admits pending Scheduled work. A recovered Claim acquires
only a retirement obligation and cannot execute its graph again. Current native
resource reconstruction rejects changed contracts before graph effects. Already
adopted configuration remains authoritative until native adoption or a fresh host
binding; a bare file edit does not silently replace that snapshot. Cancellation
and total deadlines join native work before releasing ownership. Missing physical
proof remains recovery_blocked, retaining capacity and Session deletion protection.
Bash/external owners without a supported retained positive proof remain blocked.
Native reads/cancel/reconcile never accept a force or caller-supplied proof.

Root model Schedule Tools borrow fresh native Human authority using Goal's source
selection. Runtime notifications within that request preserve its scope and used
budget. Independent Runtime activation, history, Continuation, children and fixed
Workflow nodes cannot open a grant. Eight successful mutations share the persisted
request budget; canonical Tool occurrences distinguish reused provider call IDs.
Goal consumption is independent. Provenance and budget do not prove the model
interpreted scheduling intent correctly.

Workflow terminal bodies/effects/results follow the 30-day/200-record display
retention while compact start identities survive. The native pressure fixture
mixes Completed, pre-Claim Cancelled and RejectedBeforeStart outcomes across 201
starts, verifies 200 retained payloads and preserves all deduplication identities.
The partial retained-payload index keeps cleanup independent of compact history.
A 60,000-identity SQL reproduction fell from over 200,000 VM steps to about 1,200
steps after restricting traversal to that verified index.

## Checks

| Check | Result |
| --- | --- |
| Optimized all-feature native library | 4780 passed, 8 explicitly ignored |
| Scheduler execution/recovery/Human authority focus | 26 passed |
| Independent contracts | 31 passed |
| Real CLI/App Server process suite | 65 passed |
| Strict Clippy | all targets/all features, `-D warnings`, passed |
| Rust formatting and diff whitespace | passed |
| Generated protocol and shared/TUI/Web type checks | passed |
| TUI with native binary and provider emulator | 1195 passed, 1 macOS test skipped |
| Web full suite | 3237 passed, 4 platform tests skipped |
| Web source/dependency provenance | 311 source records and 287 production notices verified |
| Web internationalization and production build | passed |
| Native resource acceptance | Schema 81 sparse matrix and independent 10-pass/600-second idle gate passed |

Release binaries and both supervisors use all features. Web fixture paths under
`target/debug` point to those release binaries; RUSTX_BINARY selects the same
native program. Provider emulator dependencies use their frozen offline environment.
Ignored allocation measurements run separately when required. Browser screenshot,
macOS, NTFS and network-filesystem acceptance are outside this Linux run.

The native checks use:

```sh
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --offline --release --lib \
  --all-features -- --test-threads=4
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --offline --release \
  --test contracts --test process --all-features -- --test-threads=4
cargo clippy --offline --release --all-targets --all-features -j2 -- -D warnings
cargo fmt --all -- --check
```

The client checks use their local TypeScript compiler, Node test runner and
Vitest with the native release program. Resource measurements, fixed budgets,
raw sparse/idle outputs and their host limitations are recorded in
[performance acceptance](issue-85-scheduler-performance.md).
