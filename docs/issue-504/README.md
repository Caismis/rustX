# Selected-Workspace MCP draft preparation

Issue: https://github.com/Caismis/rustX/issues/504

Base: `1395faf0d00f474f5a79d809b7a91d6fc99c679b`. The dedicated worktree
started from the then-latest `origin/main`,
`c3d7f0f4a105417c065efc8a0b82561b5a1901e0`, after the revised #503 merged,
and was rebased after #505 landed. Final verification includes those current
preview and native file-observation changes. The dedicated worktree and branch
are retained. This change creates one non-draft PR against `main` and does not
merge it.

## Ownership and boundaries

| Owner | Responsibility and linearization point |
| --- | --- |
| Web `DraftPreparations` | Application-lifetime selected draft, Host/endpoint and generation correlation; follows the XState route, independent of React mounts. |
| Native `McpDraftScope` | One current unsent draft and coalesced latest replacement per native connection. Sealing, cancel/close and validated transfer serialize on its control mutex. |
| Native `McpPreparationOwner` | Existing physical MCP source machinery, extracted from the coordinator: cancellation, four source slots, inactive lifecycle admissions, immutable bindings, invalidation and retirement registry. |
| Native `McpDraftRegistry` | Process-wide 32-source reservation; live ready/initializing/retiring batches stay counted. Settlement failure retains capacity and diagnostic evidence. |
| Session creation and loading | Durable create acknowledgement remains authoritative. Submitted create registers the unique counted loading flight under the catalog visibility lock; a terminal failed submission remains bounded until explicit recovery. No MCP-dependent discovery wait delays the create acknowledgement. |
| Capability coordinator | Consumes the actual pending task/live generations only after target input validation and transfer. Complete candidate validation, Workflow/child admission and capability commit still publish execution authority. |

The original native Send receipt precedes configuration resolution, remaining
capacity queueing and loading. The effective TOML setting remains the shared
`[mcp_startup_policy] wait_timeout_ms = 15000`, with CFG3 complete-object User/
Workspace precedence and positive checked deadlines. Selection alone has no
elapsed prewarm deadline. Already-ready/no-MCP cases add no artificial wait.
Fallback or ownership transfer never extends the original deadline. A known
terminal failed initialization remains a bounded residency result. Explicit user
`session/retryInitialization` replaces only that failure with a fresh clock;
automatic attach/reconnect cannot start a new initialization. The recovery action
never creates another Session or replays the retained first message.

The obsolete Web `create -> submissionIntent -> attach` sequence is replaced by
`create(submission) -> attach`. Other inert Session creation and existing-Session
submission intent remain distinct supported operations, not compatibility paths.
There is no temporary probe/reconnect path, live connection pool, durable draft
Session, prewarm TTL, UI timer or prewarm-specific visible state.

## Native and frontend mapping

| Frontend intent/state | Native meaning |
| --- | --- |
| New draft without authorized Workspace | No preparation; no Session. |
| Selected Workspace, composing | One connection-scoped native preparation; no elapsed owner deadline. |
| Workspace replacement | Old eligibility revoked immediately, old task cancelled and settled; latest intent coalesced. |
| First Send / creating | Original receipt captured; exact draft reserved; durable creation is separately acknowledged. |
| Attaching / uploading / admitting | Validated resources transferred or normal initialization joined under the original deadline. Input is still noncanonical until ordinary native admission. |
| Route departure/disposal | Cancel exact untransferred resources; a transferred Conversation remains independently owned. |
| Definite rejection / explicit discard | Cancel the old unconsumed generation and return the logical draft to editable preparation ownership. |
| Failed/uncertain submission | Retain the exact native result/continuation and original deadline; no automatic initialization or create/input replay. |

## Invariant coverage

| Invariant | Authoritative proof / boundary acceptance |
| --- | --- |
| Unsent preparation creates no Session/provider request and has no age deadline. | `draft_prewarm::unsent_ready_draft_has_no_session_or_deadline_and_transfers_one_live_catalog`; real browser ready cases independently list zero Sessions and model requests before Send. |
| Pending and prepared work are physically reused. | `sixty_seconds_preparation_then_submission_has_the_full_budget_on_the_same_task`; browser pending/ready paths each record one stdio process, one initialize and one tools/list, with the exact first model tool schema. |
| One receipt, one absolute deadline through queue/handoff/fallback. | `capacity_queueing_uses_the_same_submission_deadline_without_starting_a_process`, `submission_timeout_survives_handoff_and_cannot_resurrect_late_ready`, `cancellation_before_validated_transfer_uses_normal_startup_under_the_original_deadline`; existing accepted-node/unpolled-receipt/cold-attach/unload-handoff contracts. |
| The revised #503 policy remains authoritative. | Existing config/defaults/CFG3/schema/read/write/provenance and MCP startup suites run unchanged in FULL verification. This issue adds no second timeout setting or Web timer. |
| A/B/A is bounded, isolated and latest-only. | `latest_workspace_wins_and_ready_unclaimed_resources_retire_before_replacement`, pending switch, tab-isolation and capacity tests; desktop/mobile browser switches verify old peer/supervisor death, A=2/B=1 handshakes and ordered File upload destinations. |
| Configuration/credentials/physical Workspace cannot alias a stale owner. | Native captured binding equality, changed-binding and ready/queued directory-identity regressions. `credential_rotation_retires_preparation_while_model_only_changes_reuse_it` proves one handshake after a model-only change versus two after credential rotation, with the selected model in the first provider request and redacted diagnostics. |
| Cancel versus transfer has one winner. | Before-transfer fallback keeps the original deadline; after-transfer cancellation and connection close cannot reach consumed resources. The native scope mutex serializes reservation, cancellation and validated transfer. |
| Visibility cannot publish two runtime owners. | Expanded `issue422_creation_binding_precedes_multi_client_visibility`: submitted/inert create, known/uncertain durability, one composition and the exact same incarnation. |
| Failed creation/composition and unknown replies preserve ownership. | Failed pre-visibility creation leaves no Session and settles the claim; committed creation with a durability warning retains its original preparation/clock; repeated pre-attach failure tests retain one result and no new MCP handshake. First-submission owner tests preserve decoded durable ACKs, Files and uncertainty without create/input replay. |
| Explicit recovery is usable and bounded. | Native consecutive failed submission/recovery retries and successful recovery; Web Open Session authorization/retry/attach; TUI committed-create failure then explicit reselection. Automatic reconnect never emits the recovery command. |
| Obsolete UI work cannot retarget current navigation. | Delayed Host admission/retry-reply versus a newer Workspace route; rejected/discarded versus uncertain seal tests; application-lifetime owner tests for rerender/remount, Host replacement and carrier generation. |
| Transfer does not bypass complete capability validation. | The coordinator consumes actual generations through its ordinary full candidate and capability commit. Required Workflow/child, optional Root isolation, catalog collision/schema/pagination and stale-publication suites remain in the full native run. |
| No visible prewarm chrome; draft input survives. | Real browser cases cover en/zh at 1440/390 pixels, compare pending/ready composer content and capture screenshots. Switch cases preserve text and two ordered Files and prohibit pre-Send upload/create/input. Existing draft/model/Host-admission suites cover the retained first-submission destination. |
| Native disconnect/drain owns physical settlement. | Real stdio peer and interactive-supervisor PIDs are checked absent after switch/fixture drain; native scope/drain joins task completion, retirement and retained-capacity diagnostics. Existing spawn/handshake/retirement boundary suites run in FULL verification. |
| Every current wire consumer upgrades together. | App Server v58 schema/fixtures/TypeScript drift and type checks, Web/TUI/launcher suites and native negotiation reject obsolete peers; RuntimeClient remains v77. |

## Verification and measurements

The [executed validation receipt](evidence/validation.json) records commands,
counts, native target results and binary/source hashes. The
[independent review](independent-review.md) found no remaining actionable code
issues after repairs.

Local full runs use `TMPDIR=/run/user/1000/rustx-504-tests-rmhzf8qx`,
`CARGO_BUILD_JOBS=2` and `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Web/TUI tests use an
immutable copy of the all-feature executables at
`RUSTX_BINARY=/tmp/rustx-504-final-bins/rustx`, with their sibling supervisors.
Browser acceptance builds the default-feature binaries through the canonical
script's `cargo build --manifest-path ../Cargo.toml --bins --example
document_artifact_fixture --locked`. Client concurrency limits select the same
complete suites without filtering cases.

| Check | Exact command (from the repository root unless specified) | Result |
| --- | --- | --- |
| Rust full | `cargo test --all-targets --all-features --no-fail-fast -- --test-threads=4` | 5497 passed, 16 existing opt-in cases ignored |
| Native repair/startup regression | `cargo test --lib --all-features -- --test-threads=4 a_draining_host_refuses_attach_before_waiting_for_the_session_catalog host_request_owner_outlives_dropped_protocol_waiter issue422_creation_binding_precedes_multi_client_visibility configuration::mcp_readiness` | 41 passed; includes all 15 draft preparation cases |
| Web full | `pnpm --dir web-console test --maxWorkers=4` | 3462 passed, 4 skipped; 224 files passed, 1 skipped |
| TUI full | `pnpm --dir tui exec node --test --test-concurrency=4 'test/**/*.test.ts'` | 1303 passed, 1 platform skip |
| MCP browser acceptance | `pnpm --dir web-console test:e2e mcp-prewarm.spec.ts mcp-startup.spec.ts` | 12 passed in 46.7 seconds |
| Browser full | `pnpm --dir web-console test:e2e` | 424 passed in 26.6 minutes |
| Rust formatting / strict lint | `cargo fmt --all -- --check`; `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| Generated protocol | `pnpm --dir protocol/app-server check`; `pnpm --dir protocol/app-server typecheck` | Passed |
| Client types | `pnpm --dir web-console typecheck`; `pnpm --dir tui typecheck` | Passed |
| Translation / provenance | `pnpm --dir web-console check:i18n`; `pnpm --dir web-console check:provenance` | Passed; 312 source records and 287 production-package notices |
| Test lane coverage | `cargo run --all-features --example check_test_lanes` | Passed on Linux |
| Launcher | `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Passed; 41 tests |
| Provider emulator | `uv run --frozen pytest` in `test-support/fake-provider` | 55 passed |
| CI / benchmark tooling | `python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v`; `python3 -m unittest discover -s benchmarks/runtime -p 'test_*.py' -v` | 64 / 37 passed |

Whitespace is checked across the complete change with `git diff
1395faf0d00f474f5a79d809b7a91d6fc99c679b --check`. Final-head FULL CI, its run
URL and exact final commit SHA are recorded in the PR description; this avoids
changing the verified commit merely to record its own SHA.

The Browser plugin is unavailable in this session. Rendered verification uses the
repository's canonical Playwright workflow and pinned Linux browser container.
Local execution uses Node 24.19.0, pnpm 11.13.1, Rust 1.98.1 and the required
provider emulator. CI owns its configured Node 24.22.0 and Linux/macOS matrix.

The host root volume exhibited heavy I/O wait during simultaneous debug linking:
existing PTY/document tests exceeded their watchdogs, and the existing child-model
inheritance case exceeded its 30-second liveness bound. That same native boundary
passed in 2.23 seconds with a private executable tmpfs fixture directory. Local
full reruns use that temporary directory and two Cargo build jobs. This changes
fixture placement; assertions and timeouts are not relaxed. FULL CI supplies the standard runner filesystem
and platform coverage. Partial, interrupted and failed invocations are not counted
as passing verification.

### Measurement controls and limits

These samples come from the unfiltered full browser run. Each ready/immediate
case records one peer process, one `server/discover` negotiation, one initialize,
one initial `tools/list`, and one first model request with the exact
`startup_echo` schema. Negotiation and discovery use the same retained process.
Each A/B/A sample records A=2 and B=1 process/initialize/list cycles. The abandoned
A and B peers and supervisors are absent before the final submission; the final
A peer and supervisor are absent after fixture drain. The two ordered Files are
uploaded only to the final Session.

| Scenario / raw receipt | Selection to observed Ready or upper bound (ms) | Send to first model request (ms) | Old A cancel to both PIDs absent, upper bound (ms) |
| --- | ---: | ---: | ---: |
| [Ready, 1440px, en](evidence/ready-1440-en.json) | 373.4 | 637.6 | — |
| [Ready, 1440px, zh](evidence/ready-1440-zh.json) | 347.7 | 631.4 | — |
| [Ready, 390px, en](evidence/ready-390-en.json) | 400.2 | 661.4 | — |
| [Ready, 390px, zh](evidence/ready-390-zh.json) | 404.0 | 674.3 | — |
| [Immediate Send](evidence/immediate-send.json) | ≤885.8 | 787.5 | — |
| [A/B/A, 1440px](evidence/switch-1440.json) | Not sampled | 775.1 | ≤219.5 |
| [A/B/A, 390px](evidence/switch-390.json) | Not sampled | 839.6 | ≤235.7 |

All four pending/ready screenshot pairs are byte-identical; their
[dimensions and SHA-256 receipts](evidence/screenshots.json) accompany the eight
PNGs. Composer text and bounds are also asserted equal. The screenshots were
visually inspected across both locales and widths.

Native deadline proofs use controlled monotonic time and barriers. Their elapsed
test duration is not a latency measurement. Browser measurements use one real
stdio peer per preparation and a deliberately gated, single-page `tools/list`.
The ready cases observe native diagnostics before Send. The immediate case sends
without waiting for discovery and records the peer response-flush receipt
separately. Observing the first model request provides an upper bound for native
readiness; independent receipt delivery does not reveal its exact timestamp.

Each browser sample briefly holds the committed creation reply while assigning a
manual Session title, suppressing auxiliary title requests. Send-to-model timings
include that controlled acknowledgement/rename interval, uploads where applicable,
browser/Host work and observation overhead. Switch settlement is an upper bound
from observed cancel dispatch until the old peer and supervisor are both absent.
These are reproducible fixture samples, not percentiles or production benchmarks.
The overlapped cost is process startup, negotiation and initial catalog discovery
during composition; MCP networking itself is unchanged.

Prewarming reserves at most 32 sources per App Server. An oversized batch falls
back to normal submission initialization. Unproven retirement retains capacity
and diagnostics. An untransferred preparation is ephemeral to its native
connection; a later connection prepares afresh. There is no durable prewarm cache.
Credentialed external-provider tests retain their opt-in policy, and macOS-specific
coverage is supplied by FULL CI.
