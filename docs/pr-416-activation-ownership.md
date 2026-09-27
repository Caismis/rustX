# PR #416 activation ownership repair

This repair starts at `e438dfc939c8427df3e4514100a9175f861ffa7f` on
`issue-411-jobs-continuable-subagents`, in `rustX-issue-411`. The fetched main
`6ff49deb4c0855e64f0acaed85ebea2b6a277103` was already an ancestor.

## Recovered workspace physical authority

Recovered acquisition previously called synchronous `inspect_recovered` before
supervised retained verification. Its Git processes lacked continuation authority,
and a positive physical allocation without Reserved only advanced the ID counter.
It did not exclude reuse of the Agent workspace after restart.

All recovered acquisition Git now uses the existing supervised workspace runner,
under the new activation's physical lease. The exact current HEAD/status is passed
to retained verification, preserving deterministic allocation, repository,
canonical path, registration, branch and HEAD checks. The activation cancellation
signal reaches every verification command. Fresh workspace acquisition also
installs physical authority before Git. Agent orphan reconciliation does not run
Git; verification belongs to the owned admission path.

Reserved remains the durable semantic admission point. Startup additionally
reconstructs physical obligations from every allocation in a durable Agent's
namespace, including allocations without Reserved. Such an obligation excludes
workspace admission until exact main/helper receipts and exclusive leases prove
settlement. No synthetic Reserved or rollback event is invented for an activation
that never admitted. The physical receipt is sealed and proof locks are held
through workspace release. IDs consume both journal and physical allocations.

A helper joined by its parent may have a Quiescent receipt while a concurrent fork
briefly retains a CLOEXEC descriptor. Preparation waits for that exact lease, then
rereads the receipt under the lock. Startup probing stays nonblocking; Running,
missing or invalid evidence remains unresolved. `.pending-*` allocations grant no
spawn permit. Cleanup retains the physical evidence until Session deletion.

`recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop`
uses the production send-message/recovered acquisition path, an on-disk parent
store, and real Git status blocked inside an fsmonitor helper on named pipes.
The crash case aborts the admission owner before Reserved, reopens SQLite and a
fresh workspace manager, rejects another activation, then consumes positive
helper settlement and releases the workspace exactly once. Reusing the ID fails.
The cancellation case sends no release byte: the existing activation signal must
settle Git and return the Agent to Inactive without committing an activation.

## Absorbing child cancellation

Cancel consumed in terminal observation could be forgotten when the same loop
later received an older Completed observation. Pending guidance then caused
SealOpen, but the parent refused reopening and the child waited for a second
Cancel.

One explicit child cancellation fact now spans terminal observation, close,
seal/reopen and result selection. A stale terminal cannot authorize another turn.
The ordinary runtime cancellation path still owns current-attempt cancellation;
the child activation ends Cancelled and retains pending canonical guidance.

`cancel_before_completed_observation_never_reopens_activation` withholds a real
Completed observation, durably accepts pending guidance, delivers the only
Cancel and observes its consumption, then releases Completed. It checks normal
Cancelled protocol completion, no seal probe, exactly one initial model request,
unchanged pending SQLite input, and adoption by one later explicit activation.
Existing close/reopen and pre-Delegate regressions exercise the other phases.

## Guidance delivery evidence

Each admitted Guidance carries a shared write-start fact independent of its
semantic acknowledgement and registry ticket lifetime.

| Evidence | Result |
| --- | --- |
| Owner/control lost before the write boundary | `AgentControlError::NotDelivered` |
| Write attempted, acknowledgement lost | `AgentControlError::DeliveryUnknown` |
| Actual `GuidanceResult::Refused` | `SubagentSteerError::ChildRefused` |
| Actual `GuidanceResult::Accepted` after canonical commit | successful send |

Settlement and cancellation cannot retroactively rewrite acknowledgement evidence.
There is no replay mechanism. Native send-message emits `delivery: "unknown"` or
`"not_delivered"`; Runtime Client and App Server preserve their existing typed
errors. TUI retains the actual slash-command draft until success, preserving newer
text; Web retains failed-send drafts. English/Chinese presentation distinguishes
unknown delivery and no delivery without assuming cancellation caused every failure.

Production-path regressions cover explicit refusal, accepted acknowledgement,
real child durable acceptance followed by injected acknowledgement loss, and
control-owner loss before a frame can be written. Editor-level TUI and Web tests
check draft retention and unchanged request counts.

## Locking and current schemas

Physical allocation/fsync, recovered Git and path inspection, proof file reads,
flock acquisition, SQLite commits and process settlement execute outside the
registry mutex. Short in-memory admission/resume reservations and recovery-inflight
claims pin the exact generation; revalidation precedes publication. Workspace
exclusion uses its own lease. Proof locks remain held through the consuming
journal append or the release of a never-admitted physical obligation.

Runtime Client v52, App Server v26, child IPC v29 and SQLite schema 47 are unchanged.
No wire shape or persisted format changed. The current App Server generator
updates v26 schema descriptions; TypeScript and fixtures regenerate unchanged.
No legacy aliases, migrations, bypasses, duplicated cancellation mode or input
retries were added. The Web source provenance hash records the changed dictionary.

## Local validation

All commands below ran locally on Linux in the repair worktree. The Rust suites
ran sequentially; the provider emulator was mandatory in both boundary runs.

| Command (repository root unless noted) | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features workspace` | 151 passed |
| `cargo test --lib --all-features subagent` | 330 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,170 passed; 2 existing maintenance tests ignored; bin/example harnesses passed |
| `cargo test --test contracts --test provider --all-features` | 28 contracts and 166 provider tests passed; 5 existing live-credential tests ignored |
| `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | 193 passed |
| `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 419 passed across seven targets |

| Working directory | Command | Result |
| --- | --- | --- |
| `test-support/fake-provider` | `uv sync --frozen` | Passed |
| `test-support/fake-provider` | `uv run --frozen pytest` | 51 passed |
| `protocol/app-server` | `pnpm install --frozen-lockfile` | Passed |
| `protocol/app-server` | `pnpm check` (runs `pnpm generate`, Rust schema export, TypeScript generation and drift check) | Passed |
| `protocol/app-server` | `pnpm typecheck` | Passed |
| `tui` | `pnpm install --frozen-lockfile` | Passed |
| `tui` | `pnpm typecheck` | Passed |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 879 passed |
| `dev` | `pnpm install --frozen-lockfile` | Passed |
| `dev` | `pnpm typecheck` | Passed |
| `dev` | `pnpm test` | 37 passed |
| `web-console` | `pnpm install --frozen-lockfile` | Passed |
| `web-console` | `pnpm typecheck` | Passed |
| `web-console` | `pnpm test` | 1,082 passed in 59 files |
| `web-console` | `pnpm check:i18n` | Passed |
| `web-console` | `pnpm check:provenance` | Passed: 143 records, 131 dependency notices |
| `web-console` | `pnpm build` | Passed; existing Vite large-chunk warning |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e` | 110 passed in the pinned browser container against the final Rust binaries |
| repository root | `git diff --check` | Passed |
| repository root | `git diff --cached --check` | Passed |

The final formatting, Clippy and binary build were repeated after completing the
pre-write canonical-inbox regression and the preparation contract comment. All
passed. The App Server drift check regenerated the current files against the
staged schema descriptions; generated TypeScript and fixtures stayed unchanged.

Development runs caught stale message expectations and a proof-lock release race;
those were corrected. One earlier full run stalled during overlapping validation
and was stopped; it is not counted as a pass. The final sequential Rust run above
completed. No race ordering was changed to sleeps, no timeout was increased, and
no test was newly ignored.

The two existing library ignores are the instrumented stage profile and the
fixture-corpus writer. Five existing provider ignores require live OpenAI or
Anthropic credentials. The emulator-backed suites ran with their required flag.
macOS execution remains for CI; new filesystem/pipe operations use the existing
Unix platform contract and introduce no Linux-only imports.
