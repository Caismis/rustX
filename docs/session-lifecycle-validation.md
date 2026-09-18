# SESSION-01 implementation and validation

Issue: [#359](https://github.com/Caismis/rustX/issues/359).

## Repository and review base

- Original checkout: `/home/caismis/Documents/codes/rustX`, clean `main` tracking `origin/main`; left unchanged.
- Dedicated worktree: `/home/caismis/Documents/codes/rustX-issue-359`.
- Branch: `issue-359-session-lifecycle`.
- Fetched/rechecked base: `5d0d9ae998577cfa00a07fab1610bd1be3dce127` (latest `origin/main` when reviewed).
- Inspected #359, #287, #288, #291, #356 and PR #361 alongside current source, generated artifacts, tests and CI.
- The PR records the exact delivery head. No legacy protocol or UI compatibility path is retained.

## Ownership and deletion flow

Durable Session existence belongs to the catalog. Client attachment belongs to the
connection/view. Process-local residency belongs to `SessionRuntimeManager`.
Opening implicitly reuses, joins or composes a runtime. Closing a view only detaches.

A confirmed delete executes in a server-owned task retaining host request admission:

1. Insert the Session ID in the manager registry's `retiring_sessions` fence.
2. Join Loading/Unloading, or claim retirement of the resident incarnation.
3. Drain admitted operations, invoke native shutdown, drain projections, and release the composition only after proven settlement.
4. Inspect a fresh finite ownership target under `OwnershipSnapshot`.
5. Acquire separate `DeletionExclusion` guards for its sorted Conversation allocations.
6. Compare the confirmed semantic revision while the ownership freeze and exclusions remain held; reject stale confirmation or genuine retained-resource blockers.
7. Commit the catalog's existing atomic removal/pending-cleanup publication and durability barriers.
8. Drop catalog/root/target guards and run the existing frozen cleanup workset.

Inspection no longer acquires or retains destructive allocation exclusion. A preview
can therefore describe an attached, focused, active or nonresident Session. Its
ownership snapshot is released before a confirmation dialog is shown. Final target
derivation precedes exclusion acquisition under one continuously retained ownership
freeze; revision comparison and commit happen after exclusion, with no ownership gap.

Unproven native shutdown leaves the exact Unloading entry, composition/allocation,
counted residency slot, terminal flight failure and Session fence intact. It cannot
commit deletion or admit a replacement writer. Dropping a request does not abandon
manager transitions or reopen the fence. Safe stale/resource rejection after proven
retirement may release admission; durability uncertainty retains it. After a successful
delete, native allocation admission checks durable catalog membership, preventing
resurrection even if physical cleanup remains pending.

## Linearization points

| Boundary | Authority and exact cut |
| --- | --- |
| Load claim | Allocation-free durable identity resolution, then registry mutex: Session admission check, `by_session` claim and `Entry::Loading` installation. Only the registered flight acquires ConversationAccess. |
| Runtime publication | `TerminalGuard::finish`, under that same mutex: fence check and Loaded/ready publication. A fenced candidate instead enters Unloading on the original flight. |
| Runtime operation | `ManagedRuntimeClient::admit_operation`: same mutex checks fence/current incarnation and increments the operation lease count. |
| Attach | Operation admission plus same-mutex current/fence validation and pin acquisition; existing host route publication/connection-close cut remains authoritative. |
| Loaded → Unloading | Manager registry claim; native coordinator Running → Draining then owns execution cancellation/settlement. |
| Replacement | Same registry claim and terminal publication. Branch switching internally retires the addressed incarnation then performs normal fenced admission for the selected node. |
| Idle eviction | Existing epoch-validated native idle claim and registry transition; deletion joins that flight. |
| Delete admission | Session-ID insertion in `retiring_sessions` under the registry mutex, including when no Conversation entry exists. |
| Preview | Ownership snapshot acquired before finite durable graph/revision derivation; no allocation exclusions retained. |
| Destructive authority | Last successful sorted `ConversationExclusion` acquisition, after managed writer retirement. |
| Logical/durable deletion | Existing catalog generation CAS and atomic rename publish live removal plus frozen cleanup record; file/parent barriers prove durability before cleanup. |

No registry lock spans native shutdown, storage traversal or cleanup. Load/replacement waiters acquire no allocation handles.
No App Server mutex or UI state grants deletion authority.

## Deterministic concurrency evidence

All runtime race ordering uses actual gates, channels, watches or native notifications;
no sleeps establish a race winner. Liveness timeouts only fail tests.

| Required case | Test and parked boundary | Winner and invariant |
| --- | --- | --- |
| Resident idle / external attachment | `another_connection_deletes_an_attached_idle_session_and_closes_its_route`: first establish viewer attachment, then another connection previews/deletes; observe authoritative Closed. | Delete retires the writer, commits, closes the viewer route, and rejects its old target. No public unload or ordinary InUse. |
| Focused Session | TUI `deleting the focused last Session…`; Web `focused deletion fences controls…` holds the delete response while inspecting composer state. | Confirmation disables controls; authoritative completion removes the view, opens existing B or shows empty/New Session. No replacement Session creation. |
| Active native attempt | `deletion_settles_an_active_native_attempt`: provider response is parked after request arrival. Native shutdown-arrival and Running → Draining notifications are installed. | Delete invokes native drain while provider remains parked; successful completion releases the runtime before durable removal. No bypass of native settlement. |
| Operation first | `admitted_async_operation_drains_before_delete_releases_resources`: operation parked after manager lease admission, before native dispatch. Watch observes retirement waiting for leases. | Operation owns pre-delete settlement; deletion cannot release resources early. A later operation is rejected. |
| Delete first / operation, attach, open | `deletion_fence_rejects_late_operation_attach_and_replacement`: deletion parked immediately after Session fence, before shutdown. | Late submission, attach, load and replacement cannot enter the old runtime. Releasing the gate permits deletion. |
| Absent vs load | `deletion_fence_before_load_prevents_allocation_acquisition`: park immediately after Session fence with no registry Conversation entry. | Delete wins; late load composes zero runtimes. B remains usable. |
| Loading publication | `deletion_fence_prevents_loading_publication_and_isolates_other_sessions`: composition parked after Loading claim; a second load joins; deletion parked after its fence, then composition released. | Delete wins publication. Original and joining loads return no usable writer; the candidate drains on the existing flight, deletion succeeds, and B remains usable. |
| Replacement already claimed | `deletion_joins_replacement_without_publishing_its_candidate`: replacement parked after Unloading claim, before native shutdown; deletion then wins the Session fence. | Delete joins the transition; replacement's candidate cannot publish a usable incarnation. No second writer. |
| Replacement after delete | `deletion_fence_rejects_late_operation_attach_and_replacement`: late replacement issued while the delete fence is parked. | Replacement claim is rejected under the same mutex. Existing ordinary replacement tests retain publication-first and stale-incarnation coverage. |
| Unproven retirement | `deletion_retirement_failure_retains_writer_slot_and_session_fence`: force the native settlement failure before requesting deletion. | Failure is returned; durable Session remains, Unloading slot remains counted, and fresh load/replacement fails. No timeout or returned error fabricates proof. |
| Real ownership blockers | `deletion_workspace_transition_is_stale_and_preview_blockers_are_typed`, nested/borrowed workspace and disposal tests. Native ownership facts and disposal gates determine the order. | Retained workspace/resource ownership still prevents unsafe deletion; no automatic workspace destruction. |
| Stale preview | `deletion_preview_releases_guards_execute_reacquires_and_rejects_stale` and `deletion_stale_control_response_requires_a_new_preview_token`: add an owned child after preview, before commit. | New ownership wins; old revision returns Stale, sources remain, and another preview/confirmation is required. |
| Lost delete response | Web `lost delete response is read on reconnect…`: hold mutation reply, close socket, then provide preview/not-found/cleanup-pending/durability-uncertain read results. Existing TUI workflow/reconnect tests preserve unknown outcomes. | Exactly one delete is sent; reconnect reads authoritative state and only the live case may reattach. No mutation replay. |
| Unrelated Session isolation | Loading-publication and absent-fence tests load/use B while A is parked behind its deletion fence; existing conformance runs real A work while B ownership inspection is held. | A's runtime transition does not serialize B; only the existing brief shared ownership/catalog boundaries are shared. |

The existing one-writer, single-flight, bounded-residency, idle-clock, native
settlement, route independence, publication durability and server-drain suites remain.
Old resident-preview failure expectations now test inspection separately from
actual destructive exclusion.

## Protocol and clients

App Server **v7 → v8**, strictly. `rustx.app-server.v8` is the only WebSocket
subprotocol. Rust initialization, TUI initialization/decoder/transport, Web
initialization/transport, fixtures, schema generation and drift checking all use v8.
The previous version is rejected, with no fallback.

- Removed `session/unload`, its `unloaded` result, list `residencies`, and deletion blockers `current_session` / residency-only `in_use`.
- Added semantic `session/switchNode`. The speculative `session/restart` method was removed during review because neither product client consumes it. Internal manager replacement and safe live configuration reload remain.
- Generated `protocol/app-server/v8.ts`, `v8.schema.json`, `fixtures.ts`, `fixtures.json`; removed v7 files under the single-current-version convention.
- TUI: removed `/unload`, registration/help/completion, selector residency badges and switch-away deletion restriction; branch confirmation says “Switch to this branch”. Focused deletion and reconnect verification use native outcomes.
- Web: removed public unload/advanced residency controls and `sessionResidencies`; Close view only detaches, Open Session implicitly acquires residency. Current deletion disables controls and removes the view after authoritative completion. Settings uses semantic Open/Reload wording.
- Inspector retains attachment/incarnation and observed runtime facts; explicit server diagnostics retain internal residency. Harness-first Sidebar/navigation and naming are preserved.

Vocabulary review classified remaining Loaded/Loading/Unloading/Unloaded references
as internal manager/diagnostic contracts or their safety tests. Removed-method strings
remaining in client tests are negative assertions, not registrations or compatibility
paths. Native Runtime Client protocol 39 → 40 removes the obsolete delete/recover
mutations with strict version rejection. Its read-only preview remains. UUIDv7
identity formats are unrelated to protocol versioning.

## Initial implementation validation (reviewed head e2e1ce69)

Commands run from the dedicated worktree unless a directory is shown. Repeated
checks below report their final result; the development failures are recorded afterward.

| Command | Result |
| --- | --- |
| `git fetch origin` | Passed; reviewed base unchanged at the SHA above. |
| `cargo check --all-targets --all-features` | Passed. |
| `cargo check --lib --all-features` | Passed. |
| `cargo fmt --all` | Passed. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test --all-targets --all-features` | Passed: 3,702 tests, zero failures, six pre-existing ignored tests. Includes real App Server process/scripted tests. |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests` | Passed: 88 manager/scripted/transport tests. |
| `cargo test --test process --all-features app_server` | Passed: 13 real App Server process tests. |
| `cargo test --lib --all-features local_runtime::session::tests::deletion_tests` | Passed: 52 deletion tests after splitting inspection/exclusion expectations. |
| `cargo build --bins` | Passed; real binaries used by TUI/browser/process acceptance. |
| `pnpm --dir protocol/app-server install --frozen-lockfile` | Passed. |
| `pnpm --dir protocol/app-server generate` | Passed. |
| `pnpm --dir protocol/app-server check` | Passed; regenerated artifacts equal the staged reviewed artifacts. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir tui install --frozen-lockfile` | Passed. |
| `pnpm --dir tui typecheck` | Passed. |
| `pnpm --dir tui test` | Passed: 788 tests. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Passed: 788 tests, zero skipped. |
| `pnpm --dir web-console install --frozen-lockfile` | Passed. |
| `pnpm --dir web-console typecheck` | Passed. |
| `pnpm --dir web-console test` | Passed: 432 tests across 29 files. |
| `pnpm --dir web-console build` | Passed; Vite reports its existing large-chunk advisory. |
| `pnpm --dir web-console check:provenance` | Passed: 104 source records and notices for 100 production packages. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Passed: all 39 tests against real App Server/Product Host. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh shell.spec.ts --update-snapshots` (Web directory) | Passed: eight tests; only confirmation/Inspector references changed after visual inspection. |
| `pnpm install --frozen-lockfile && pnpm typecheck && pnpm test` (dev directory) | Passed: 30 launcher tests. |
| `uv sync --frozen --directory test-support/fake-provider` | Passed. |
| `uv run --frozen --directory test-support/fake-provider pytest` | Passed: 51 tests. |
| `git diff --check` | Passed. |

The browser plugin was unavailable, so browser QA used the repository's pinned
Playwright workflow and immutable Linux container, without changing tolerances.
Reviewed [delete confirmation](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-delete-light-linux.png)
and [Inspector](../web-console/test/e2e/shell.spec.ts-snapshots/desktop-right-panel-linux.png)
references preserve the current layout. Full acceptance exercises responsive/keyboard
navigation, shared-server TUI, real native work, branch switching, focused deletion,
configuration reload and reconnect.

Development checks initially found obsolete unload/residency test expectations,
old v7 process initialization payloads, client type/fixture mismatches, a focused-TUI
presentation recursion, and an overly strict post-publication load check. These
were corrected and the affected suites rerun. Initial browser checks also caught
provenance hashes, two intentional screenshot changes and an ambiguous status
selector. A later run differed by one antialiased pixel at the unchanged composer
border; its baseline/tolerance was not changed, and subsequent full runs passed.

The six ignored Rust tests are five credential-dependent live provider probes and
the explicit committed-fixture regeneration test. They were not executed or claimed
as passing. Linux CI-equivalent checks ran locally; macOS CI was not run locally.
No required Linux check was blocked by the environment. No known implementation
follow-up is required for #359.

## PR #362 ownership review corrections

The load owner resolves Session/node identity without allocation access, then checks
admission and atomically installs the Session claim and Loading flight under the
registry mutex. Only the registered transition acquires ConversationAccess. Warm
loads and flight joiners acquire none. Replacement follows the same boundary.
There is no managed allocation that deletion cannot identify through the registry.

Flight terminals are `Resident`, `WriterAbsent(operation_result)` and
`RetirementUnproven(error)`. Native shutdown, projection drain and composition
release establish the old writer-transfer proof. New composition failure preserves
that proof; deletion can continue despite the failed operation. Shutdown uncertainty
retains the exact Unloading slot and Session fence. No error-based registry inference
or deletion retry substitutes for writer certainty.

The obsolete native SessionDelete/SessionDeleteRecover requests, mutation mapping,
attachment handling and supervisor dispatch are removed. The manager is the only
production caller of SessionController's durable delete primitive. Other direct
callers are explicitly low-level durability/ownership tests; the similarly named
MCP transport operation concerns a remote MCP session, not a rustX Session.

The speculative App Server `session/restart` API has no product caller and was
removed from v8 and generated artifacts. Existing internal targeted replacement,
branch switching and safe live reload retain their owners. Process acceptance now
proves persisted selection and current-source composition across process restart.

| Deterministic test | Parked boundary, winner and proof |
| --- | --- |
| `load_claim_is_visible_to_delete_before_allocation_acquisition` | Load parks after registry claim and before ConversationAccess. Delete joins that flight; the second load is a joiner, acquisition count is zero while parked, B remains usable. Release yields exactly one acquisition/composition, no usable writer publication and successful durable deletion. |
| `deletion_fence_before_load_prevents_allocation_acquisition` | Delete parks after absent-Session fence. Later load is rejected with zero acquisitions/compositions; B remains usable. |
| `deletion_joining_failed_loading_receives_proven_writer_absence` | Composition parks while holding allocation under Loading. Delete joins before injected failure. Flight returns WriterAbsent(Err), allocation/slot release precedes successful deletion, and no retirement fence remains. |
| `deletion_joins_replacement_failure_after_proven_writer_transfer` | Replacement parks after shutdown, projection drain and old composition release, before reacquisition. Old native weak handle is gone. Delete joins; new composition fails, terminal is WriterAbsent(Err), deletion commits, and no fence remains. |
| `deletion_retirement_failure_retains_writer_slot_and_session_fence` | Forced native settlement uncertainty returns RetirementUnproven, retains the old slot and fence, preserves durable Session, and rejects load/replacement. |

The operation/attach, Loading/replacement publication, active-work settlement,
current deletion, workspace blockers, stale confirmation and lost-response tests
listed above remain in the full acceptance suites. The three new tests and two
strengthened tests assert transition facts, counts and terminal outcomes without sleeps.

Self-review: no managed acquisition is invisible to deletion; managed loads cannot
produce ResourceConflict through an unregistered transition; only unproven writer
retirement retains the fail-closed runtime fence; no native product delete mutation
bypasses the manager; all publication/retirement paths preserve one writable incarnation.

### Revision validation

All commands below ran in the existing issue worktree. The latest fetch retained
base `5d0d9ae998577cfa00a07fab1610bd1be3dce127`; no rebase was necessary.

| Command | Final result |
| --- | --- |
| `git fetch origin` | Passed; main unchanged. |
| `cargo check --lib --all-features` | Passed. |
| `cargo check --all-targets --all-features` | Passed. |
| `cargo fmt --all` | Passed. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test --all-targets --all-features` | 3,705 passed, zero failures, six existing ignored tests. |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests` | 91 passed. |
| `cargo test --lib --all-features local_runtime::session::tests::deletion_tests` | 52 passed. |
| `cargo test --test process --all-features app_server` | 13 passed. |
| `cargo build --bins` | Passed. |
| `pnpm --dir protocol/app-server generate` | Passed; v8 TypeScript/schema regenerated. |
| `pnpm --dir protocol/app-server check` | Passed; no drift from staged artifacts. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir tui typecheck` | Passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 788 passed, none skipped. |
| `pnpm --dir web-console typecheck` | Passed. |
| `pnpm --dir web-console test` | 432 passed in 29 files. |
| `pnpm --dir web-console build` | Passed; existing chunk-size advisory. |
| `pnpm --dir web-console check:provenance` | Passed; 104 source records and 100 package notices. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 39 passed against real App Server/Product Hosts; unchanged pinned browser references. |
| `pnpm --dir dev typecheck` | Passed. |
| `pnpm --dir dev test` | 30 passed. |
| `uv run --frozen --directory test-support/fake-provider pytest` | 51 passed. |
| `git diff --check` | Passed. |

Intermediate checks caught remaining references to the removed restart method and
old Flight Result access, a singleton fixture loop rejected by Clippy, and two
future-version rejection expectations still using native version 40. Those were
corrected; the checks and full Rust suite above were rerun successfully. No tests
were skipped, timeouts increased or synchronization weakened to obtain a pass.
The existing six ignored tests remain the credential probes/fixture regeneration
listed above. macOS validation runs in CI, not this Linux worktree.

## Recovery ownership review correction

Public `session/recoverDeletion` now runs through SessionRuntimeManager in a
server-owned task retaining host request admission. It uses the existing finite
durable preview before granting recovery authority. Live Preview/Blocked results
leave any active delete or uncertain-writer fence untouched. Only committed-record
observations invoke SessionController::recover_deletion. Observed absence confirms
catalog persistence separately; it does not acquire cleanup authority.

Deleted, confirmed NotFound, and CommittedCleanupPending release the provisional
manager fence idempotently. Durable absence/retired identities or the durably
published frozen record now reject runtime/allocation resurrection. Continued
CommittedDurabilityUncertain retains the fence. A remaining managed writer slot
cannot be overridden by durable recovery. Runtime Client v40, App Server v8 and
all wire shapes remain unchanged.

| Regression | Exact boundary and outcome |
| --- | --- |
| `recovery_observes_live_session_without_releasing_active_delete_fence` | Delete parks immediately after Session fence installation, before durable handoff. No committed record exists. Recovery returns Preview with the same revision; original fence remains, load is rejected with zero acquisitions. Releasing delete commits normally. |
| `recovery_settles_durability_uncertainty_without_runtime_reconstruction` | Inject post-rename catalog failure after resident writer retirement. Delete returns CommittedDurabilityUncertain with fence retained. Recovery returns Deleted, clears the fence and leaves composition count at one. Load remains rejected by durable absence; B stays usable. Repeated recovery returns confirmed NotFound. |
| `recovery_with_unproven_catalog_durability_retains_delete_fence` | Inject post-rename delete failure, then pre-rename recovery publication failure. Recovery returns CommittedDurabilityUncertain; frozen record and fence remain, load is rejected, acquisitions/compositions stay zero. |
| Existing admitted-operation/delete race | Operation parks on its admitted lease while deletion drains it. Public App Server recovery returns Preview, retaining the active delete fence and old writer until native settlement. |
| Existing unproven-writer regression | Recovery returns live Preview after failed native settlement; it does not clear the fence or admit a replacement writer. |
| TUI lost-response regressions | One execute loses its reply. Unknown has no recovery capability/footer and R emits no cleanup request. A subsequent fresh server-confirmed committed preview can enable explicit recovery without replaying delete. |

`deletion_duplicate_recovery_cannot_regress_terminal_or_mint_a_second_snapshot`
already proves that duplicate/original cleanup work shares frozen authority and a
delayed worker failure cannot turn finalized absence back into pending cleanup.
No extra recovery operation registry or timing-based ordering was introduced.

Web audit: reconnect still requests `session/deletePreview` before reattachment;
there is no Web `session/recoverDeletion` caller. TUI preserves unresolved observation
when its notice closes, but unknown alone grants neither cleanup action nor replay.

Self-review answers are all **No**: public cleanup recovery without committed
observation; NotFound for a live Session; live observation releasing an active
delete fence; successful recovery permanently retaining the uncertainty fence;
still-uncertain recovery reopening admission; lost response granting cleanup authority.

### Recovery revision validation

Fetched origin before final validation; main remained at
`5d0d9ae998577cfa00a07fab1610bd1be3dce127`. No rebase was needed.

| Command | Result |
| --- | --- |
| `git fetch origin` | Passed; base unchanged. |
| `cargo fmt --all` | Passed. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test --all-targets --all-features` | 3,708 passed, zero failures, six existing ignored credential/fixture tests. |
| `cargo build --bins` | Passed. |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests` | 94 passed. |
| `cargo test --lib --all-features local_runtime::session::tests::deletion_tests` | 52 passed. |
| `cargo test --test process --all-features app_server` | 13 passed. |
| `pnpm --dir protocol/app-server check` | Passed; invokes generation internally, no artifact drift or wire changes. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir tui typecheck` | Passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 789 passed, none skipped, including rebuilt-server integration. |
| `pnpm --dir web-console typecheck` | Passed. |
| `pnpm --dir web-console test` | 432 passed in 29 files. |
| `pnpm --dir web-console build` | Passed; existing chunk-size advisory. |
| `pnpm --dir web-console check:provenance` | Passed; 104 source records and 100 package notices. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 39 passed; unchanged pinned browser references. |
| `pnpm --dir dev typecheck` | Passed. |
| `pnpm --dir dev test` | 30 passed. |
| `uv run --frozen --directory test-support/fake-provider pytest` | 51 passed. |
| `git diff --check` | Passed. |

An initial TUI run exposed that dismissing unknown had discarded its observation
context; it now preserves observation without granting recovery. A Clippy doc-markdown
failure was corrected. Both suites were rerun successfully. No timeout, screenshot,
race synchronization or coverage was relaxed. Linux validation used the repository's
pinned browser container; macOS remains a CI check, not a local claim.
