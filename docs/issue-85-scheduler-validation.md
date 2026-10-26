# Scheduler integration validation

The candidate integrates origin/main `9c794f763` with the Scheduler branch,
including MCP Resource source authority and its frozen approval policy.
Current formats are App Server 60, Runtime Client 79, SQLite 82, Session catalog
18, child IPC 37 and Scheduled Workflow compiler contract 3. Older stores and
peers are refused without migration or compatibility readers. Web and TUI share
the sole generated v60 schema/TypeScript pair. The recorded integration and
performance evidence below predates this merge and retains its original versions.

The integration preserves Scheduler writer ownership while adding the native
submission-intent and initialization-retirement contracts: a first submission
waits for a cold Scheduler writer's physical release before claiming Loading;
scheduled activation registers an exact-node initialization and shares its
startup receipt and cancellation owner. Host drain closes both admission paths.
Published runtime unload joins admitted operation leases before retiring native
admission; Loading cancellation retains the immediate initialization fence.
Accepted cold Workflow and Message targets arm the shared startup budget after
their exact pending-target recheck. If an ordinary load wins Loading first,
composition recognizes Accepted Workflows through the bounded pending query
and arms the original load receipt before MCP preparation. A Scheduler join
cannot renew that deadline; Claimed no-replay recovery alone remains unarmed.
Job/Agent waits transfer native completion
tails outside manager leases while retaining Host capacity and physical proof.

The measurements and broad checks below were recorded on Ubuntu 24.04 / WSL2
ext4 with Rust 1.98.1, Node 24.21.0 and pnpm 11.13.1. Exact-head FULL CI and
current integration results are linked in PR #2. Immutable performance evidence
retains its recorded source commit and is not presented as a new measurement.

## Verified behavior

Both temporal targets use the native conversation writer: Runtime Message
admission and pinned Scheduled Workflow admission. Accepted-to-Claimed issues one
execution capability; Tool, Model, Agent and Review nodes share the existing
Workflow executor and physical owners. The Scheduled run and interaction scope
contains no synthetic Root Attempt, ToolCall or model turn. Live and historical
interaction audit projections preserve absent Attempt/turn metadata.

Native cold loading admits pending Scheduled work. A recovered Claim acquires
only a retirement obligation and cannot execute its graph again. Current native
resource reconstruction rejects changed contracts before graph effects, including
Agent primary and explicit summary provider model IDs while accepting credential
rotation. Invalid arguments and denied Workflow permissions return typed command
refusals and leave valid reminder commands and due admission available. Already
adopted configuration remains authoritative until native adoption or a fresh host
binding; a bare file edit does not silently replace that snapshot. Cancellation
and total deadlines join native work before releasing ownership. Missing physical
proof remains recovery_blocked, retaining capacity and Session deletion protection.
Bash/external owners without a supported retained positive proof remain blocked.
Native reads/cancel/reconcile never accept a force or caller-supplied proof.
Reconcile preserves a registered owner's terminal publication authority after its
graph closes; a completed graph retains its exact Return value and digest.

The manager dispatches never-served and then least-recently-served sources, keeping
service age across summary refreshes. A controlled-clock fixture parks 16 native
Loading flights and requires the healthy due source's committed admission before
releasing them. A separate SQLite busy-writer fixture retains a real pending
target while checking activation and another Session's load/unload progress.
Loaded activation uses a notification handle without taking composition under the
global registry.

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

The Chat viewport preserves reading intent when committed stream content shrinks
and grows before one layout frame. It samples reader movement before the DOM
commit and acknowledges browser clamping afterward; a single RAF still owns
automatic positioning. Eight deterministic React regressions cover following,
detached anchors and user movement before/after commits. Sampling queues the
detached correction before its state update can overwrite a pre-clamp anchor;
newer user movement supersedes that correction. A pending gesture blocks
automatic movement immediately; once sampled, the 25px tail tolerance determines
following. Desktop and narrow gesture acceptance enforce the same geometry.

Checkpoint capture reserves the bounded Goal CAS descriptor before history and
display bodies consume the shared inline allowance. Dense history cannot remove
a current Goal or its controls from Web/TUI. The native regression retains the
Goal reference and phase under pressure while enforcing the existing checkpoint
byte bound.

Browser fixtures wait for a committed Fork target and lineage, resolve a reopened
file's native absolute path before writing it, and require a cancelled native
outcome after explicit Stop. Only that deliberately stopped provider step permits
disconnect; source-running and independent Fork assertions remain enforced.
The native image scenario waits for uploaded-message execution to settle before
reconnecting and submitting its next message. It verifies the new attached
Conversation and positive native inherited boundary before decoding copied images.

The 38 updated existing screenshot references add only the Schedules entry in
the Session header. They were captured with the unchanged pinned Playwright
1.63.0 Noble inputs from FULL run `40292207615`, source head `eacd9bf9e`, and
matching merge `0143fbcdf`. Capture run `40294954979` retained four PNGs in shard 1
and 34 in shard 2. Both archive SHA256 values, the source manifest and every PNG
digest were verified; every changed pixel is within the top 40px header row.
The exact comparator and noise policy remain unchanged.

## Integration checks

The current integration passes 92 native Resource cases, 43 checkpoint cases,
the deterministic cold-branch recovery/capacity case and 163 related Web tests.
The held HTTP pagination matrix covers Scheduler-first and ordinary-load-first
Workflow/Message startup, unchanged receipt deadlines, cancellation with physical
settlement, and unarmed Claimed recovery without graph replay. All 65 focused
scrolling/preview/Fork tests and both reading viewport browser cases pass.
Web types, the production build, internationalization, 322 source provenance
records and 309 production dependency notices pass. Strict Clippy covers every
target and feature. Seven focused browser cases pass, including native Goal
control, independent Fork image roundtrip and both viewport widths. FULL CI must
additionally verify the final PR head on the
complete Linux/macOS and pinned browser surfaces before merge; PR #2 records
the exact run and head rather than reusing an earlier head's result.

## Recorded broad checks

| Check | Result |
| --- | --- |
| Optimized all-feature native library | 4787 passed, 8 explicitly ignored |
| Scheduler execution/recovery/Human authority and manager regressions | 31 passed within the full native run |
| Independent contracts | 31 passed |
| Real CLI/App Server process suite | 65 passed |
| Strict Clippy | all targets/all features, `-D warnings`, passed |
| Rust formatting and diff whitespace | passed |
| Generated protocol and shared/TUI/Web type checks | passed |
| TUI with native binary and provider emulator | 1196 passed, 1 macOS test skipped |
| Web full suite | 3294 passed, 4 platform tests skipped |
| Web source/dependency provenance | 311 source records and 287 production notices verified |
| Web internationalization and production build | passed |
| Browser Schedules references | 34 regenerated and reviewed under pinned Playwright 1.63.0 Noble; exact comparison and noise policies preserved |
| Chat scrolling | 4 React regressions, desktop/narrow gestures and 3 independent native reading repetitions passed |
| Provider fixtures | Python suite passed; explicit Stop and reopened-file checks passed |
| Native Fork image roundtrip | target/lineage readiness checked; original fixture and three independent repetitions passed |
| Native resource acceptance | Schema 81 sparse matrix and independent 10-pass/600-second idle gate passed |

Release binaries and both supervisors use all features. Web fixture paths under
`target/debug` point to those release binaries; RUSTX_BINARY selects the same
native program. Provider emulator dependencies use their frozen offline environment.
Ignored allocation measurements run separately when required. Complete browser
acceptance discovers 403 tests and uses two shards under the same immutable
Playwright container. The required `full-verification` PR profile enforces both
complete shards and the Linux/macOS boundary suites for the exact candidate head.
NTFS and network-filesystem acceptance remain outside this Linux run.

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
