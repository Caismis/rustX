# Issue #374 implementation and regression map

Base: `d9453ce878f098dd717e1cd8663ca1135d7f147b` (freshly fetched `origin/main`).
Worktree: `/home/caismis/Documents/codes/rustX-issue-374`.
Branch: `issue-374-subagent-conversation-navigation`.

## Architecture reviewed

The exact ownership chain is App Server `AttachmentTarget` validation → admitted
parent Runtime Client operation authority → `ConversationRuntime` → that runtime's
`SubagentRegistry.index[SubagentId]` → immutable owned child ConversationId →
existing, confined Conversation allocation access → identity-bound read-only
SQLite handle. A child ConversationId is never a request field or independent
lookup authority. Prepared/uncommitted children are not inspectable.

The private `transcript_store` seam resolves ownership; the shared Runtime Client
`read_transcript_page` helper applies existing `load_transcript_page`,
`transcript_page_view`, and `response::decorate_through` semantics. Root reads
retain their existing projection frontier; child reads capture their own durable
presentation frontier. No transcript assembly moved into App Server or TUI.
Existing allocation access prevents deletion during a read without retaining
child execution or admitting a Session. Missing stores are never initialized.

App Server v12 adds only `subagent/transcript` and the closed
`unknown_subagent`/`subagent_history_unavailable` errors (both carry SubagentId).
It reuses the existing `transcript { page }` result. v11 files are removed;
Rust DTOs, schema, TypeScript, fixtures, both clients, transport handshakes,
provenance inventory and documentation advance together. Native Runtime Client
RPC, durable database and catalog versions are unchanged.

Ctrl+Up/Down selects a native child; Enter opens its read-only transcript; `i`
retains the separate detail view. Esc returns to Main's Composer without a
request or changing its draft/cursor. The transcript has no writable Composer,
Session controls, child lifecycle controls, model/permission/Goal controls or
interaction response callback. Root-routed HITL is unchanged.

The child reader holds one page, default 32 entries. PageUp replaces it using
its exact older cursor. Home discards it and requests newest. A 1.5-second poll
reads newest transcript authority, never activity/status; older browsing pauses
polling. Polling continues for terminal children so status cannot imply a final
transcript boundary. No streaming tokens or settlement facts are fabricated.

Parent attachment epochs fence replacement, resync, closure and release. Each
child view has an immutable SubagentId and a disposable read generation. Starting
a replacement page or disposing the view invalidates its continuations; closing
also clears the render callback and timer. Popup callbacks check both exact
reader identity and parent presentation lease. Reconnect/resync preserve only the
selected SubagentId and reread through current authority; errors clear obsolete
history and display unavailability. Parent switching closes/disposes the child
view even when the old parent remains attached.

## Acceptance mapping

Test labels below are exact; the transport test runs once for `stdio` and once
for `websocket`.

| Label | File and exact test |
| --- | --- |
| N1 | `src/runtime/subagent/registry/tests/archive_ownership.rs`: `child_transcript_requires_exact_committed_parent_ownership_and_survives_terminal` |
| N2 | Same file: `failed_and_cancelled_retained_children_keep_exact_transcript_authority` |
| P1 | `src/app_server/schema.rs`: `child_transcript_has_only_exact_parent_subagent_read_authority` |
| P2 | `tests/support/app_server_conformance.rs`: `representative_scenario`, shared by direct, stdio and WebSocket conformance |
| I1 | `tui/test/integration.test.ts`: `native child transcript stdio: exact ownership, running/waiting/terminal, tools, metadata and reconnect` |
| I2 | Same file: `native child transcript websocket: exact ownership, running/waiting/terminal, tools, metadata and reconnect` |
| T1 | `tui/test/subagent-transcript.test.ts`: `child selection disposes A before B; late A success or failure cannot update B` |
| T2 | Same file: `paging replaces one bounded page and exact boundary generation rejects stale pages` |
| T3 | Same file: `refresh reads transcript authority; termination cannot invent settlement; disposal fences reads` |
| T4 | Same file: `unavailable history is explicit and clears the obsolete projection` |
| T5 | Same file: `parent detach fences child continuation; reads send exact parent and Subagent only` |
| T6 | Same file: `resync fences old child read; authoritative reread reconstructs without writes` |
| U1 | `tui/test/composer-app.test.ts`: `child inspection preserves exact parent draft and cursor; Esc sends no request` |
| U2 | Same file: `actual child A to B navigation rejects late A without Session or control requests` |
| U3 | `tui/test/app.test.ts`: `remote recovery reconstructs selected child from replacement authority without replay` |
| U4 | Same file: `switching parent Session fences a child page while the old parent remains attached` |

| Issue acceptance case | Proof |
| --- | --- |
| 1. Exact parent + Subagent resolves exactly one child | N1, I1/I2 |
| 2. Arbitrary/unrelated Conversation cannot bypass ownership | N1, P1/P2, I1/I2 (child ID as Subagent and wrong parent rejected) |
| 3. Running child readable without Session creation | N1, I1/I2 (one parent attachment at running read), U2 (exact request list) |
| 4. Terminal retained history remains readable | N1, N2 (failed/cancelled retained workspaces), I1/I2 |
| 5. Missing history explicit, never an empty success | N1, T4, I1/I2 (temporarily remove terminal database, assert typed error and no recreation) |
| 6. Older ordering/cursors | N1, T2, I1/I2 (one-entry wire pages and strict cursor ordering) |
| 7. Canonical Tool correlation | I1/I2 (exact assistant message, block index and ToolCallId) |
| 8. Exact completed-response metadata | I1/I2 (closing message, child origin and total usage 132) |
| 9. A → B stale response fencing | T1, U2 (also rejects stale popup close callback) |
| 10. Parent switch fences continuations | T5, U4 (old parent remains attached) |
| 11. Reconnect authoritative reconstruction, no replay | T6, U3, I1/I2 (real WebSocket disconnect/reconnect) |
| 12. Termination does not invent settlement | N1/N2 (history equality across registry settlement), T3, I1/I2 (no response completion at child provider gate) |
| 13. Esc makes no runtime request | U1 |
| 14. Exact draft and cursor preservation | U1 (Unicode grapheme draft plus cursor-sensitive deletion) |
| 15. Root-routed child HITL unchanged | I1/I2 (two exact routed interactions, child answered through root, inspection leaves waiters unchanged); existing native child Approval/Questionnaire regressions |
| 16. No child write/steer/control API | P1, T5/T6, U1/U2 and I1/I2 request-count/absence assertions |

Race tests use deferred promises, exact request-log barriers, registry commit and
settlement watches, or provider gates. No sleep establishes race correctness.
Native existing HITL regressions include
`child_questionnaire_routes_through_root_without_parent_mediation`,
`child_approval_allow_routes_to_child_and_runs_exact_invocation`,
`child_approval_deny_routes_to_child_and_never_starts_executor`, and
`child_questionnaire_fails_closed_without_root_provider`.

## Validation

Validation was run in the isolated worktree. Earlier iterations caught old
protocol-version expectations, a test's token-number type, and a parent Composer
focus regression; these were corrected, not waived.

| Command | Final result |
| --- | --- |
| `git diff --check` and `git diff --cached --check` | Pass |
| `cargo check --all-targets --all-features` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins` | Pass; used by real transport/provider tests |
| `pnpm --dir protocol/app-server install --frozen-lockfile` | Pass |
| `pnpm --dir tui install --frozen-lockfile` | Pass |
| `pnpm --dir web-console install --frozen-lockfile` | Pass |
| `pnpm --dir protocol/app-server generate` | Pass |
| `pnpm --dir protocol/app-server check` | Pass; regeneration matches staged generated artifacts |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir tui typecheck` | Pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 854 passed, zero skipped, including stdio and WebSocket real child inspection |
| `node --test tui/test/subagent-transcript.test.ts tui/test/composer-app.test.ts` | 20 passed |
| `cargo test --lib --all-features child_transcript` | 2 passed: native ownership and protocol authority |
| `cargo test --lib --all-features failed_and_cancelled_retained_children_keep_exact_transcript_authority` | 1 passed |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | 522 passed across 36 files |
| `pnpm --dir web-console check:provenance` | Pass; 116 source records and notices for 100 production packages |
| `uv run --project test-support/fake-provider pytest test-support/fake-provider/tests` | 51 passed |
| `cargo test --all-targets --all-features` | Latest run: library 3186 passed, 3 failed during managed Python source preparation, 1 intentionally ignored; Cargo stopped before integration targets |
| `cargo test --lib --all-features boundary_suites::runtime_client::python_capability:: -- --test-threads=1` | Both failed capability tests passed on isolated retry |
| `cargo test --lib --all-features boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority -- --exact --nocapture` | Still failed during source preparation |
| `cargo test --all-features --test cfg3_catalog --test cfg3_managed_output --test conformance --test contracts --test durable --test process --test provider --test subagent --test tools` | Catalog 30, managed output 5, conformance 23, contracts 27, durable 130, process 53, provider 166, Subagent 53 passed; provider has 5 existing ignored tests. Tools: 123 passed, 7 failed during managed dependency preparation |

The full Rust suite is **not claimed green**. Final library failures were:

- `boundary_suites::runtime_client::python_capability::capability_projection_covers_native_python_and_skills` (passed retry)
- `boundary_suites::runtime_client::python_capability::capability_projection_covers_python_origins` (passed retry)
- `boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority`

The remaining library error is `ToolActivation` / `SourceUnavailable` for
`source:python:healthy`, reason `source preparation failed`. An earlier full run
also failed `boundary_suites::mcp_mrtr_managed::a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction`:
`uv sync --frozen --no-install-project --no-default-groups --no-config` failed to
download `authlib==1.8.0` from `files.pythonhosted.org` after three retries with a
connection timeout. That test passed the next full run.

Final tool integration failures:

- `mcp_managed::a_real_managed_child_negotiates_the_modern_mcp_revision`
- `mcp_managed::cold_python_composition_rereads_sources_while_loaded_capture_is_frozen`
- `mcp_managed::one_connected_runtime_reuses_one_process_across_calls`
- `mcp_managed::one_folder_serves_multiple_tools_through_one_server_identity`
- `mcp_managed::stderr_diagnostics_do_not_participate_in_framing`
- `mcp_managed::two_folders_prepare_distinct_environment_identities`
- `uv::production_uv_materializes_a_managed_package_environment`

The tools log reports `files.pythonhosted.org` DNS errors (`Name or service not
known` / `Temporary failure in name resolution`) downloading `rich==15.0.0`,
`beartype==0.22.9`, `uvicorn==0.53.0`, `rpds-py==2026.6.3`,
`python-dotenv==1.2.3`, and `fastmcp==4.0.3` metadata. The cold-composition test
reports the resulting source-preparation failure. No dependency-dependent test
was disabled or changed to conceal these failures.

One extra focused-test invocation using `pnpm --dir tui exec tsx --test ...`
failed because this repository does not install `tsx`; the corrected native
Node command above passed. The complete TUI suite also passed after the final
code changes. Local raw logs are retained under ignored
`target/issue-374-validation/` in the task worktree.
