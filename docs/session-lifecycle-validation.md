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
| Load claim | Registry mutex: Session admission check, `by_session` claim and `Entry::Loading` installation in one critical section. |
| Runtime publication | `TerminalGuard::finish`, under that same mutex: fence check and Loaded/ready publication. A fenced candidate instead enters Unloading on the original flight. |
| Runtime operation | `ManagedRuntimeClient::admit_operation`: same mutex checks fence/current incarnation and increments the operation lease count. |
| Attach | Operation admission plus same-mutex current/fence validation and pin acquisition; existing host route publication/connection-close cut remains authoritative. |
| Loaded → Unloading | Manager registry claim; native coordinator Running → Draining then owns execution cancellation/settlement. |
| Replacement/restart | Same registry claim and terminal publication; exact-incarnation restart rechecks its target at the claim. Branch switching internally retires the addressed incarnation then performs normal fenced admission for the selected node. |
| Idle eviction | Existing epoch-validated native idle claim and registry transition; deletion joins that flight. |
| Delete admission | Session-ID insertion in `retiring_sessions` under the registry mutex, including when no Conversation entry exists. |
| Preview | Ownership snapshot acquired before finite durable graph/revision derivation; no allocation exclusions retained. |
| Destructive authority | Last successful sorted `ConversationExclusion` acquisition, after managed writer retirement. |
| Logical/durable deletion | Existing catalog generation CAS and atomic rename publish live removal plus frozen cleanup record; file/parent barriers prove durability before cleanup. |

No registry lock spans native shutdown, storage traversal or cleanup. Load/replacement
waiters drop their redundant allocation handles before awaiting the owner flight.
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
| Absent vs load | `deletion_fences_absent_session_before_a_new_load_claim`: park immediately after Session fence with no registry Conversation entry. | Delete wins; late load composes zero runtimes. B remains usable. |
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
- Added semantic `session/switchNode` and `session/restart`. Restart fully reconstructs the exact addressed branch from current configuration; it never leaves the Session merely unloaded. Existing safe live configuration reload remains unchanged.
- Generated `protocol/app-server/v8.ts`, `v8.schema.json`, `fixtures.ts`, `fixtures.json`; removed v7 files under the single-current-version convention.
- TUI: removed `/unload`, registration/help/completion, selector residency badges and switch-away deletion restriction; branch confirmation says “Switch to this branch”. Focused deletion and reconnect verification use native outcomes.
- Web: removed public unload/advanced residency controls and `sessionResidencies`; Close view only detaches, Open Session implicitly acquires residency. Current deletion disables controls and removes the view after authoritative completion. Settings uses semantic Open/Reload wording.
- Inspector retains attachment/incarnation and observed runtime facts; explicit server diagnostics retain internal residency. Harness-first Sidebar/navigation and naming are preserved.

Vocabulary review classified remaining Loaded/Loading/Unloading/Unloaded references
as internal manager/diagnostic contracts or their safety tests. Removed-method strings
remaining in client tests are negative assertions, not registrations or compatibility
paths. Local Runtime Client protocol history and UUIDv7 formats are unrelated to App
Server versioning and remain unchanged.

## Validation commands and results

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
