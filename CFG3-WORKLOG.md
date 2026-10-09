# CFG3 implementation review notes

Issue #332 and its runtime-layout/UUIDv7 amendment are one implementation.
These notes describe the final owners; the issue remains the contract.

Original checkout: `/home/caismis/Documents/codes/rustX`, clean `main` at
`a105022cf0bbcd0cf7eab501a8f4f881a55b3db9`, left untouched. Implementation:
`/home/caismis/Documents/codes/rustX-issue-332`, branch `issue-332-cfg3`.
The prior `a4a2cad` atomic-overlay checkpoint is retained in this branch.

## Authority and ownership

| Domain | Final owner and contract |
| --- | --- |
| Authored config | Strict `local_runtime::authoring::RuntimeLayer`, one `rustx.toml` per User/Workspace scope. Provider and Model identities are independent. |
| Process bindings | `launch::HostEnvironment` and `UserConfigSources`: absolute User `--config` and `--runtime-root`; fixed User resources; no ancestor/XDG/trust sources. |
| Overlay/provenance | Explicit typed units in `authoring.rs`; complete winning objects and their own defaults. No recursive TOML merge. |
| Effective resolution | `configuration::UserConfigManager` captures both config files and resource roots; no saved effective document or Session reconstruction path. |
| Credentials | Provider winner and source admission resolve their own references. Safe source projections redact literal secrets and cannot retain a lower-scope credential. |
| Resources | Existing Skill/Agent/Workflow/MCP/Python readers reserve higher identities before parsing. Invalid winners remain invalid, unused diagnostics are ordered/bounded. |
| Profiles | `runtime::agent_profile`, Root config, complete named-Agent resources. Child model inherits the invoking Attempt snapshot; no Root Tool/Plugin ceiling. |
| Tool policy | Existing invocation-policy and Tool-plane owners; Agent selection never changes execution/concurrency/approval policy. |
| External lifecycle | Existing `CapabilityCoordinator` prepares finite admitted Root/child/Workflow demand. Python captures bytes inertly; definitions alone have no effects. |
| Generation | `ConversationRuntime` coordinator owns an immutable `RuntimeResourceSnapshot` containing `RuntimeConfiguration`, catalog, bindings, policy, profiles and prepared capabilities. |
| Session | `SessionPersistentState { cwd, model }`: Workspace selection and optional explicit model intent only. |
| Authoring CAS | `configuration/settings.rs` and native source lock/revision primitives; canonical whole-document TOML writer, no browser merge. |
| Protocol | App Server v6, generated from Rust; source/effective/write/reload operations, typed reload errors. Internal projection protocol v38 remains a separate native boundary. |
| Clients | TUI calls native commands; Web owns presentation/drafts with Effective/User/Workspace. Both reconstruct observations without replaying Save/Reload. |
| Storage | `SessionCatalog`, `runtime::local_storage`, typed UUIDv7 identities, explicit ordinals, one SQLite DB per Conversation. |
| Tool output | `ManagedToolOutput`, Conversation-owned results/tasks, UUID names, create-new/no-overwrite, no TTL or OS-temp stable locator. |

The complete schema, overlay matrix, default behavior, startup grammar and
filesystem tree are in [configuration.md](docs/configuration.md). Structured
client behavior is in [web-settings.md](docs/web-settings.md).

## Publication and failure boundaries

`ConversationRuntime::reload_configuration` takes the coordinator lock, refuses
pending interaction, active Attempt, compaction, owned background/child work or
another reload, and closes admission. `LocalRuntimeResourceLoader::prepare`
rereads both config files and both `.agents` roots, resolves typed overlay and
whole-resource shadowing, calculates Root demand and prepares sources off-side.

After all fallible candidate/Session-model/context validation, the coordinator
lock protects capability commit, Session model rebinding, global child policy
publication and `state.resources = Arc::clone(&resources)`. That assignment is
the generation publication point. A single `ConversationObservation::Resources`
carries the coherent model/policy/resource cut; there is no intermediate public
capability observation. Admission reopens only after publication. Failure or
cancellation before commit preserves the exact old generation Arc. Physical
retirement failure after commit is owned by runtime health and cannot report
that a committed generation reverted.

Admitted Attempts and child/Workflow specs own frozen snapshots. The safe/busy
contract refuses publication while those owners are active. Session explicit
model intent is revalidated against new bindings and survives default changes.
Cold composition always rereads current files and revalidates deliberate intent;
Save never mutates the loaded generation and restart needs no previous reload.

Source commits hold the cooperating writer lock, check the exact original
revision, validate a typed mutation, canonicalize complete TOML, write/fsync a
same-directory stage, recheck the source revision and rename, then fsync the
parent. Rename is the visibility point; post-rename durability uncertainty is
reported separately. External edits observed at either fence invalidate stale
writes. No claim is made of filesystem CAS against an uncooperative editor
writing inside the final check/rename interval. Clients preserve conflicts and
repair uncertain responses by rereading, never automatic mutation replay.

## Identity and deletion

Session/Node/Conversation/ToolExecution are strict `ses_`/`node_`/`conv_`/`exec_`
UUIDv7 types at durable and public boundaries. UUID identity is independent of
catalog admission ordinals, child ordinals, registry order and journal sequence.
Allocation injects UUID generators in tests, checks registry/durable/path absence,
serializes cross-Session Conversation reservations, and uses exclusive directory
or create-new file allocation. Bounded retry or explicit refusal preserves prior
bytes, including orphan allocations. Retired catalog identities remain reserved.

`~/rustx/runtime/sessions/catalog.json` owns Session graph metadata. Every Session
owns `conversations/conv_<uuid-v7>/conversation.sqlite`, plus Conversation-owned
`tool-output/results/result_<uuid-v7>.txt` and
`tool-output/tasks/exec_<uuid-v7>.output`. Accepted and terminal background output
share that exact execution locator. Reconstruction never scans output names to
seed a sequence. Session deletion uses the existing access/deletion transaction
and cleans owned DB/output; independent forks retain their own ownership.

Development schema versions are config 9, catalog 11, SQLite 38, App Server 6,
internal Runtime Client 38 and child IPC 26. Old formats are refused, not migrated.

## Deterministic evidence map

| Invariant | Native/client regression boundary |
| --- | --- |
| Complete atomic overlay/default provenance, absent/empty/value | `authoring` and `authoring_cfg3_tests`; `cfg3_catalog` Provider/Model/Plugin/native whitelist tests |
| No lower credential splice; redacted source authoring | `cfg3_catalog::provider_replacement_cannot_borrow_lower_credential`, `provider_secrets_are_not_read_back_and_cannot_be_retained_from_lower_scope` |
| Two scopes/roots and inert old files | `split_files_are_inert_and_config_rebinds_only_user_document`, `config_and_runtime_root_are_absolute_process_bindings` |
| All five malformed Workspace shadow families | `cfg3_catalog` Agent/Skill/Workflow/Python/MCP tests plus unused-invalid collection regression |
| Skill prompt visibility/root paths | `skills_all_exact_and_empty_are_valid_for_both_agent_kinds`, `skill_prompt_exposes_collection_roots_without_enumerating_absolute_package_paths` |
| Independent named profile/frozen parent model | `named_profile_is_independent_and_child_inherits_the_frozen_invoking_model`; real subagent process suite |
| Inert external discovery and admitted demand | `an_uninvoked_named_agent_does_not_connect_its_mcp_source`; `scripted_suites::capability::python_tools::child_source_admission_begins_python_preparation_using_the_frozen_package_capture`; counters and captured package edits |
| Off-side candidate/cancellation/one publication | `settings_e2e::cfg332_complete_candidate_stays_offside_cancellation_keeps_old_and_publication_advances_once`; native coordinator gates |
| Save vs Reload vs cold reread; explicit model | `settings_e2e::cfg332_save_is_cas_only_reload_publishes_and_cold_resolution_rereads`; `live_generation_changes_only_on_complete_reload_and_preserves_explicit_model` |
| All four source groups / no disk-only mutation | `resource_revisions_cover_both_complete_roots_without_implicit_publication` |
| Busy/frozen work/endpoint change | `conversation_runtime` reload tests; host activation/semantic-commit gates; real browser busy test |
| Source CAS/one winner/external edit/uncertain write | `cfg3_catalog` stale/competing-writer tests; scripted App Server publication faults; browser recovery response interception |
| Named authoring validates before commit | `named_agent_save_validates_complete_definition_before_committing`; real browser valid-definition assertion |
| UUIDv7/collision/order/no restart reuse | `session::tests::cfg3_identity`, `cfg3_managed_output`, typed protocol rejection tests, durable recovery suites |
| Conversation/fork/deletion ownership | Session deletion/process-death durable suites and `headless_composition_allocates_session_owned_uuid_conversation_database` |
| Background/foreground stable locators | `cfg3_managed_output`, managed-output and background recovery tests; real tool/process suites |
| Effective/User/Workspace, full structured settings | Web component tests plus real `settings.spec.ts`, `integrations.spec.ts`, `workflow.spec.ts` |
| Reconnect/no replay/lost acknowledgments/drafts | `recovery.spec.ts`, actual native App Server and transport interception; TUI reconnect suites |
| Responsive/keyboard/native Session controls | 4 browser viewport keyboard cases, console/commands/composer/workspaces acceptance; full TUI suite |

Tests use owner hooks, channels, barriers, counters and injected identities for
semantic interleavings. Existing bounded process liveness timeouts are not race
proofs. Correctness runs use the fake provider and local fixtures; live paid
provider tests remain intentionally ignored. Linux validation is local; macOS
filesystem/process coverage remains in CI, including both CFG3 integration targets.

## Removed architecture and audit exceptions

Deleted trust stores/actions/epochs/UI, split User settings/model readers and
flags, MCP activation enum/`enabled`, Session Tool/Skill narrowing, configurable
Skill source roots, old source settings/integration DTOs, V5 schema and decoding,
resource-only public reload, and obsolete examples. `CurrentRuntimeConfig` is the
internal resolved type, not an authored source or a renamed compatibility reader.
`UserConfigManager` is the one CFG3 composition/authoring owner; it no longer reads
split files. Legacy filenames/flags remain only in negative rejection/inertness
tests. Internal Agent/subagent/Attempt/message ordinals remain where ordinal
semantics are real; they are not public Session/Conversation/ToolExecution IDs.

No dynamic Plugins, migration framework, Workflow language redesign, distributed
configuration/locks or full Skill/Python/Workflow editor was added.
