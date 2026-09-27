# PR #416 integration with #422

The integration starts at PR head `943d596b409c066ed721825c804e936e41d30c04`
and merges main `ad863a24cf48fbb0d5182746e1046d4b64e8c167`, retaining both
histories. The old merge base was `6ff49deb4c0855e64f0acaed85ebea2b6a277103`.
No feature branch replacement, compatibility path, or runtime migration is added.

## One protocol lineage

App Server **v27** is the sole supported generation. Mainline v24 introduced
mandatory `session/summaryInvalidated.catalog_changed`; branch-local v26 added
Jobs and durable Agents without that field. Neither was the combined contract.
The new generation incorporates both and rejects earlier peers. Runtime Client
remains v52; its shape did not need a new version for this integration.

The Rust generator owns schema/fixtures, and `protocol/app-server/generate.mjs`
owns TypeScript bindings. Only `v27.schema.json` and `v27.ts` remain. TUI, Web,
WebSocket subprotocol, initialization, fixtures, download scripts and checks use
v27. Historical repair reports retain the versions they actually validated.

The catalog storage owner records membership changes at visibility commit,
including committed-but-durability-uncertain outcomes. Metadata publication records
false; creation, copies and deletion record true. Coalescing retains the last
membership sequence independently of the last metadata sequence. The connection
forwards that evidence unchanged. Web invalidates the named summary and rereads
membership only for true. TUI has no live summary/catalog cache: notifications do
not mutate Conversation state, and every `/resume` catalog opening reads native
membership again. Neither client guesses the flag from local cache membership.

## Integrated ownership

Native creation validates and commits Session identity/configuration without
composing a runtime. Web first-input ownership transfers from the draft binding to
the exact acknowledged Session synchronously before navigation. ACK observers are
isolated from RPC settlement; a presentation exception cannot strand a request.
The request pump preserves both the bounded request lanes and the startup
`dispatchCurrent` fence. A retired queued startup operation never crosses transport.
Deletion remains coordinated by the native Session owner; client navigation does
not commit creation or infer successful deletion. Controller recovery retains
`work.settle().await` for exact physical retirement, while creation installs its
captured configuration binding before catalog visibility, with pre-visibility
rollback under the existing native ownership guards.

Jobs remain finite executions. Durable Agent identity survives finite activations.
Registry selection captures the reserved/current activation atomically for wait
and interrupt. Resume cannot overlap an existing reservation or unproven physical
obligation. Accepted, not-delivered, delivery-unknown and explicit refusal remain
distinct; neither client replays an uncertain request.

TUI retains lanes of wait 4, admission 2, control 2 and ordinary RPC 8, bounded
before request-ID allocation. Its per-attachment `AppServerSession` submission
fence owns one explicit Agent-message submission until classification. An old
attachment's completion cannot release a replacement's fence. Exact editor text
comparison prevents late success from clearing newer typing. Interruption remains
available while message admission is pending.

Web retains wait 4, admission 2, control 2, ordinary transmitted RPC 8 and its
bounded ordinary queue. `agent/wait`, `job/wait`, `agent/sendMessage`,
`agent/interrupt` and `job/cancel` have domain-owned response lifetimes. Ordinary
RPCs retain the 30-second transport deadline. Actual socket loss settles pending
operations once, preserves mutation uncertainty and never replays them. Connection
and attachment fences reject stale adoption.

Physical identity consumption, authority publication and logical facts remain
separate. The allocation namespace lock excludes recovery while a live initializer
could publish. Private ordinal creation is synced before authority initialization;
complete locked lease/receipt authority is atomically renamed and the parent synced
before a handle escapes. Recovery seals abandoned private identities as consumed;
published evidence still requires exact lease/receipt/identity proof. Missing or
corrupt published evidence never authorizes idle, reuse, drain or deletion. Proof
locks survive the durable proof append and in-memory settlement cut. One
reconciliation pass owns each exact claim; a concurrent caller cannot discharge it.
These are filesystem ordering guarantees, not claims that a task-abort test models
power loss.

Ordinary child results still select the final canonical Assistant MessageId from
the exact concluding durable AttemptId, including the last accepted-guidance
attempt. Refusal-only completion cannot borrow historical text. Workflow children
keep their finite output-latch plus durable successful-terminal contract, including
success after late cancellation and cancellation before output commit.

The native portability repairs remain intact: short independently owned control
socket namespaces, valid filesystem identities separate from opaque bytes, exact
binary Git output, and non-consuming stopped observation using
`WSTOPPED | WNOHANG | WNOWAIT`.

## Deterministic integration regressions

- Rust generated fixtures cover both mandatory catalog flags; TypeScript rejects
  the old flag-less shape. The parked display-publication test explicitly asserts
  false after its release gate. Existing creation/deletion tests assert true at
  the visibility commit, including committed-uncertain deletion and coalescing.
- `tui/test/catalog-invalidation.test.ts` delivers each flag through the actual
  client decoder, verifies no Conversation mutation, and uses request-log barriers
  to prove each catalog opening issues a fresh read.
- `a replacement attachment does not inherit an old Agent-message submission
  fence` holds the old RPC, detaches, attaches a new target and starts its own RPC.
  Releasing the old response cannot admit a duplicate on the new attachment.
- Web `explicit catalog scope controls rereads even for a Session absent from
  local state` uses response barriers and held catalog reads to distinguish false
  from true without deriving membership from the local view.
- Web `full wait and RPC lanes preserve controls and retired startup dispatch
  ownership` holds four waits and eight reads, rejects excess wait admission,
  completes interrupt/cancel, retires startup authority, then releases read slots.
  The queued startup never sends; outstanding waits still complete normally.
- Existing actual-composer repeated-Enter, request-lane saturation, exact recovery
  claim channel, unpublished allocation boundaries, canonical child-result,
  Workflow ordering and stopped-observation tests run again after integration.

## Validation

Final command outcomes are recorded below after execution. Linux evidence does not
constitute macOS evidence. The existing `macos-latest` job remains enabled with its
native boundary coverage; no platform exclusion, skip, assertion weakening or
longer deadline was introduced. Hosted macOS must execute against the delivered
integrated commit before merge readiness can be assessed.

Executed on Linux with Rust 1.98.1 for the Rust matrix/strict Clippy. Commands run
from repository root unless a directory is specified. All results here are from
integrated source, not reused pre-integration results.

| Command | Final result |
| --- | --- |
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | passed, no warnings |
| `cargo build --bins` | passed; these native binaries were used by TUI/browser tests |
| `cargo test --lib --all-features subagent` | 337 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,190 library tests passed, 3 existing ignores; bin/example targets passed |
| `cargo test --test contracts --test provider --all-features` | contracts 28 passed; provider 166 passed, 5 existing live-provider ignores |
| `protocol/app-server`: `pnpm check`, `pnpm typecheck` | passed, including final regeneration/drift repeat |
| `tui`: `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | passed; full suite 893 passed, no skips |
| `web-console`: `pnpm typecheck`; `pnpm test` | passed; 1,094 tests across 60 files |
| `web-console`: `pnpm check:i18n`; `pnpm check:provenance`; `pnpm build` | passed |
| `web-console`: `CONTAINER_ENGINE=podman pnpm test:e2e` | 126 passed, no skips, 7.8 minutes |
| `dev`: `pnpm typecheck`; `pnpm test` | passed; 37 tests |
| `test-support/fake-provider`: `uv run --frozen pytest` | 51 passed |

Browser authority is
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
No screenshot baseline was updated.

Focused commands also passed:

```sh
cargo test --lib --all-features recovered_physical_receipt_requires_owner_release_and_durable_proof
cargo test --lib --all-features supervised_settlement_git_preserves_large_binary_output_and_argument_bytes
cargo test --lib --all-features stopped_anchor_supervisor_is_contained_by_the_outer
cargo test --lib --all-features native_control_paths_are_bounded_and_independently_owned
cargo test --lib --all-features clean_native_settlement_survives_abandoned_parent_terminal_publication
cargo test --lib --all-features stopped_observation_preserves_stop_and_terminal_status_for_owner
cargo test --lib --all-features physical_recovery::inheritance_tests
cargo test --lib --all-features persistent_child_result_belongs_to_concluding_terminal_attempt
cargo test --lib --all-features deletion_retires_abandoned_unpublished_activation_without_inventing_an_agent
cargo test --lib --all-features group_proof_cannot_consume_shell_exit_between_hygiene_passes
```

Each exact native filter ran one test; the physical inheritance module ran six.
From `tui`, `node --test test/app-server-client.test.ts test/composer-app.test.ts
test/commands.test.ts test/catalog-invalidation.test.ts` passed 157 tests.
From `web-console`, `pnpm exec vitest run test/client.test.ts
test/request-lifetime.test.ts` passed 53 tests.

The three existing library ignores are the real-create stage profiler, isolated
native cold-load measurement, and committed document-fixture regeneration. The
five provider ignores require external credentials/live services. None was added
or changed by this integration.

Development failures are retained as evidence, not counted as passing validation:

- Initial Web typecheck found an unused merged `WorkspaceHostError` import; removed.
  Initial TUI typecheck found the new test's missing method discriminator in
  `paramsOf`; corrected. A root-directory `pnpm typecheck` invocation also failed
  because this repository has no root package manifest; rerun in `tui`.
- The first full TUI run started before rebuilt v27 binaries were ready: the log
  explicitly reports server v26/client v27. It ended with 875 passed, 18 failed and
  one cancelled test after its owned hanging test process was stopped. It is not
  final native evidence. A single filtered native probe passed its assertion but
  failed the suite's shared emulator completion check because sibling scenario
  steps were not run. The complete suite was subsequently rerun.
- A full TUI run then had 890 passed/3 failed due to wrongly version-renamed
  Runtime Client questionnaire fixture paths. The initial full Rust run had
  3,187 passed/3 failed: those two missing fixtures and an old-version rejection
  input accidentally changed from 26 to 27. These are separate version domains;
  restored the historical Runtime Client fixtures/negative input. Complete Rust
  library/bin/examples and complete TUI suites then passed.
- The first contracts run had 27 passed/1 failed on stale documentation saying
  v25 and earlier were rejected. Corrected to v26 and earlier; the complete
  contracts/provider command then passed. No assertion was weakened.

No macOS tests were executed locally. Hosted macOS against the final integrated
commit is pending at delivery; local Linux success is not a merge-readiness claim.

The first serialized boundary run had 193 passed/1 failed:
`fastmcp4_availability_selection_request_and_invocation_share_one_authority`
reported a selected healthy managed Python source unavailable during preparation.
The public error redacts the underlying preparation reason. Temporary controlled
fixture instrumentation on the exact test then showed the healthy source Ready
and the intentionally conflicting `pydantic<2.12` source rejected by the pinned
FastMCP dependency solver; that exact test passed. Instrumentation was removed
byte-for-byte before the complete serialized rerun. The original underlying cause
was not captured and is not classified as infrastructure or claimed fixed. No
source assertion, timeout, dependency pin or platform coverage was changed to
obtain the rerun result.

The final complete serialized boundary command passed **194 tests, zero failures
and zero ignores**:

```sh
RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::
```

This full-suite pass, after restoration of uninstrumented source, is the final
boundary evidence. The earlier managed-source preparation failure remains an
unexplained validation risk, not a silently discarded run.

The final serialized integration command passed **419 tests, zero failures and
zero ignores**: CFG-03 catalog 26, managed output 5, conformance 22, durable 129,
process 62, subagent 45, tools 130.

```sh
RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output
```

Final `git diff --check` and `git diff --cached --check` passed. All conflict
entries were resolved; only the v27 generated App Server artifacts remain. The
primary worktree was not modified (its original `.playwright-mcp/` remains
untracked). The integration uses a normal merge commit and normal push on the
existing branch; it neither rewrites history nor merges the PR.
