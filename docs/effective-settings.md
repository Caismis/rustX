# Effective settings and configuration lifetimes

rustX has several configuration authorities, not one mutable “current configuration”.
The Rust launch resolver interprets disk inputs for a prospective launch. Composition
captures a small redacted set of source facts. A running Session, approval policy,
resource generation, and admitted execution then have independent native owners.

`rustx config show --sources` describes a **prospective next launch**. `/settings`
describes the **attached native runtime**. They can legitimately disagree. Reading
`/settings`, attaching, or reconnecting never opens configuration files or applies
changes. The separate `/defaults user` operation reads the user document explicitly;
it does not refresh or replace the live snapshot.

## Field and action matrix

| Setting or fact | Owner | Source | Live mutability / application boundary | Persistence | Resource reload | Restart |
| --- | --- | --- | --- | --- | --- | --- |
| Launch model/profile and approval source facts | Rust resolver; immutable composition capture | Built-in, user, trusted project, CLI, according to field ownership | Immutable capture; **not current disk truth** | None from inspection | Capture unchanged | New resolution and capture |
| Selected Session model and generation-time reasoning profile | `SessionModelState`, coordinated by `ConversationRuntime` | Launch default for a fresh Session, or explicit persisted Session selection | Whole-state validated replacement under coordinator lock; next eligible admission | Existing Session catalog persists Session choice; `/model` never edits user/project defaults | Unchanged | Explicitly resumed Session retains its own choice; fresh Session uses resolved default |
| Effective/desired approval mode | `ConversationRuntime` | Launch policy, then explicit runtime control | Under coordinator lock: desired changes immediately; effective changes when idle, otherwise reconciliation at attempt settlement before subsequent admission | No implicit default write | Unchanged | New launch policy; old work is never resumed by a settings change |
| Ordinary selected tools and source availability | Existing capability/resource owners | Admitted activation, registration and exact selection policy | No generic live setter; bounded resources reload requires quiescence | Authoring files only | Validated coherent capability/resource replacement | New composition |
| Roles, Skills, instructions, Workflow registrations and policies | `RuntimeResourceSnapshot` and existing loader | Pinned authorized resource slots and files | Prepare off-side; publish under coordinator lock while admission is gated | Authoring files only | New published generation, or old generation on failure | New composition |
| Runtime root, model catalog/bindings, host startup policy | Launch resolver/composition | Host-owned paths and allowed CLI inputs | Restart required | Explicit authoring | Not reapplied; loader retains captured startup values | Re-resolved |
| Saved primary model/profile or approval default | Rust `UserDefaults` document writer | Explicit `user` scope: `$XDG_CONFIG_HOME/rustx/settings.toml` (or `$HOME/.config/rustx/settings.toml`) | Same-directory rename publishes one validated document | Only the explicitly selected fields | Does not replace Session model or approval | Can affect a future launch; project/CLI precedence still applies |
| Attempt model, resource revision, approval mode | Native admission | Actual frozen model/resource/policy snapshots | Frozen together at admission; never live-mutated | Retained execution/request evidence only | Retained admitted facts unchanged | No reconstruction from new defaults |
| Effective native Agent Extension composition (Agent Status) | The composed `ConversationRuntime` itself; frozen at `LocalConversationCore::compose` (root) or in `ResolvedSubagentSpec.extensions` (child) | The launch's `agent.extensions` profile dimension (root) or the named Agent profile's `extensions` dimension (child) | Immutable for the life of the Agent; there is no live setter and no install/uninstall | Authoring files only | Unchanged — reload republishes resources and cannot recompose extensions | New launch resolves the current document and may compose a different set |
| Child model/profile/capabilities and compiled/admitted Workflow program/inputs | Existing Subagent and Workflow admission owners | Parent-frozen native specifications and compiled program | Immutable for admitted execution; no rediscovery in child workspace | Existing native evidence only | Already admitted specifications unchanged | No replay or automatic resume |
| Show reasoning, expansion | TUI presentation preferences | Client-local preference | Immediate rendering change | Not saved through native settings API | No effect | Client presentation only |

A new Session created **inside an existing local host** uses that host's captured
launch defaults. Saving user defaults does not refresh this capture. A new launch
can observe the saved values; explicitly resuming a Session still respects its
persisted Session-local model selection.

## Daily controls

- `/model` opens the native catalog selector; `/model provider/model` selects a
  model for the active Session. Primary overrides reset as documented by that
  control, while the independent summary policy is preserved.
- `/model profile set <id>` and `/model profile clear` read the whole current native
  model configuration, replace its reasoning-profile selection, and submit `model_set`. The native
  binding and context validators decide whether the complete selection is valid.
  `clear` removes the explicit selection and uses the catalog default profile.
  Profile IDs are opaque: `set default` and `set clear` select those literal IDs.
  The old `/model profile <id>` syntax is rejected; there is no sentinel or alias.
- `/approval` opens an interactive picker. Policy selection requests the native
  safe-boundary transition; Full access first requires explicit confirmation
  with Cancel focused. Opening, highlighting and cancelling send no mutations.
- `/show-reasoning on|off` changes rendering only: no provider parameters, model
  selection, tool invocation or canonical history changes. `/reasoning` is unknown;
  there is no compatibility alias.
- `/settings` shows captured sources, Session selection, effective/pending approval,
  actual published resource revision, tools/Skills, the attached Agent's frozen
  native Agent Extension composition, and frozen attempt facts.
- `/defaults user` shows only the permitted saved fields, target document and its
  content revision. It does not resolve layers or claim the values win precedence.
- `/save-default user model <revision>` writes exactly `agent.model.model` and
  `agent.model.reasoning_profile` from the current native Session selection, captured by Rust
  under the coordinator lock at the save operation. The TUI sends only a target,
  never a value from its asynchronous projection cache. A cleared profile
  is written as `{ mode = "catalog_default" }`, preserving unrelated TOML and
  comments. A selected profile uses `{ mode = "profile", name = "…" }`.
- `/save-default user approval <revision>` writes exactly `approval_mode` from the
  authoritative desired runtime mode, captured under the coordinator lock. An
  active attempt can still retain a different frozen effective mode; for example,
  desired `full_access` is saved while current work remains on `policy`.

Read `/defaults user`, review the document/revision, then pass that revision to the
explicit save command. A normal model/profile/approval command never saves defaults.
A save result names the scope/document, changed field/value, new fingerprint,
`live_unchanged: true`, and `applies_at: next_launch`. No combined disk/live transaction
exists. Project writes, arbitrary JSON patches, provider edits, credentials, request
parameter persistence and general configuration CRUD are outside this interface.

`/reload` is **not** general settings hot reload. It rejects active attempts,
interactions, compaction or a competing reload as busy. During a permitted reload,
the runtime gates admission, prepares a candidate off-side, and commits the capability
and resource replacement under its coordinator lock. A failed preparation/publication
leaves the prior pair authoritative. The result and snapshot report the published
revision, never a failed candidate's revision. Startup/Session fields are not reapplied
from the resource documents. Existing schema/ownership validation still applies to
authored documents; malformed TOML cannot be ignored by resource loading.

## Commit points and evidence

- **Session replacement:** `state.model = candidate` in
  `ConversationRuntime::model_set_with_persistence`, under the same coordinator lock
  as admission, after whole-state resolution/context validation and existing
  Session-catalog persistence. This Session persistence is distinct from defaults.
- **Attempt freeze:** `publish_attempt` captures model, resource snapshot and
  effective approval while holding that lock. One `AttemptAdmitted` observation
  contains all three facts. The projection never substitutes current Session values
  while waiting for a second admission observation.
- **Approval:** the desired/effective assignments and observation under the
  coordinator lock; settlement reconciles effective to desired before another
  admission can take the lock.
- **Resources:** the existing capability commit and `state.resources` replacement
  under the coordinator lock, protected by the reload admission gate.
- **Defaults:** successful `NamedTempFile::persist` (same-directory rename) is the
  one file publication commit. Staging, file sync and validation are before it.
  No fallible directory sync is performed after publication; power-loss directory
  durability is not promised. A successful save reports the published revision. An unexpected save-worker failure
  reports publication outcome as unknown and requires rereading; it never claims rollback.
- **Reconnect:** the existing host projection lock drains native observations and
  captures snapshot/cursor for `initialize`. The client replaces its presentation
  from that snapshot, then subscribes after the cursor. No cached settings are
  restored as semantics, and no model/approval/save/reload operation is replayed.

## Effective native Agent Extensions (Issue #256)

The two configuration surfaces answer different questions, and both answers are
correct at once:

| Surface | Question it answers |
| --- | --- |
| `rustx config show --sources` | What extension configuration would a **next launch** resolve, and from which authored layer? |
| `/settings` | Which native Agent Extension composition is the **attached Agent** actually running with? |

`/settings` renders `RuntimeClientSnapshot.effective_extensions`, which the
runtime projects from the composition it already materialized
(`ConversationRuntime::native_extensions()`). Reading it opens no configuration
file, resolves no layer, and consults no resource generation. So editing
`rustx.toml` after launch makes the two views disagree — and that disagreement
is the contract, not a bug: the live view keeps describing the Agent that is
running.

- **Root launch freeze.** The root composition is resolved once, in
  `LocalConversationCore::compose`, and the projection is installed once, when
  the Runtime Client host binds over that runtime. Its lifetime is
  `launch_capture`.
- **Child resolved-spec freeze.** A child's composition is the one its invoking
  generation froze into `ResolvedSubagentSpec.extensions`, before staging and
  the durable ownership commit. The child's own Runtime Client host projects
  exactly that value, so a child frozen under resource generation R1 keeps
  reporting R1 after R2 publishes, and root extension configuration cannot
  reach it. Its lifetime is `frozen_admission` under `frozen_child` evidence.
- **Resource reload does not recompose extensions.** The root Agent's composed
  `agent.extensions` is launch-scoped. `/reload` publishes a new
  `RuntimeResourceSnapshot` but does not recompose the already-running root's
  native Extension set or mutate a child's frozen `ResolvedSubagentSpec`.
  The projection has no reload seam at all.
- **Restart may change them.** A new launch resolves the current document
  through the ordinary resolver. That is the only way a root Agent's
  composition changes, and it rewrites no canonical Session history.
- **Observations are not extension-configuration authority.** The `statuses`
  window says what was composed for a step. An extension that is enabled but
  has produced no eligible contribution yet still projects as composed, and a
  composed status never implies an extension is present.
- **Historical partial evidence is not reconstructed.** Durable-only inspection
  has no live Agent, so `effective_extensions` is `null`. It is never filled in
  from today's document, built-in defaults, or the latest runtime
  configuration, even when the inspected history itself contains canonical
  Agent Status messages.

Absence is represented once, and precisely: `agent_status: null` means the
extension is not part of this composition, which is a different fact from a
composed extension whose Time and Background contributors are both disabled.
`timezone: null` means no explicit timezone was configured — not UTC. The
projection is a closed typed record with one member per native extension; it
carries no prompts, registries, extension internals, secrets, or arbitrary
configuration documents, and it is not a generic configuration read/write
surface.

The members are **independent axes**. `todo: null` means this Agent composes no
Todo extension, and it says nothing about Agent Status — nor the reverse. It is
also the only authoritative answer to "does this runtime have a current task
list, a `todo` Tool, and a Todo panel?": a client must never infer that from
`todo` results in the transcript, which are historical facts of the
conversation rather than facts about the runtime attached to it. The same
distinction governs `RuntimeClientSnapshot.todos`, where `null` means "no Todo
extension composed" and the empty list means "composed, and this conversation
has no tasks".

Protocol 28 adds the `effective_extensions.todo` member and makes
`RuntimeClientSnapshot.todos` nullable, so "no Todo extension composed" and
"composed over an empty task list" stop sharing one wire spelling (Issue #259).
Both follow from one frozen composition and therefore cannot disagree. The
change is breaking and deliberately carries a new version number: protocol 27
is Issue #258's effective Subagent profile digest, whose `effective_extensions`
has no `todo` member. rustX is pre-1.0, so the protocol may break without a
compatibility layer — that is not licence for two protocols to share a version.
There is no v27 decoding: a v27 client is refused by strict version
negotiation. Protocol 27 adds the subagent `profile_digest`. Protocol 26 adds
`effective_extensions` and `settings_lifetimes.extensions`.
Protocol 25 extends the existing snapshot with `launch_settings`,
`settings_lifetimes`, and `settings_evidence`. Canonical model, policy and resource
sections remain the only live value projections. Attempt `model` and
`execution_settings` are nullable when native evidence is unavailable;
`attempt_started` carries the same frozen facts. The live Session model is null for
historical-only inspection. No synthetic `inspection/durable` model or “old request
= current model + current tools” fallback is used. `historical_partial` explicitly
marks unavailable live state; request-specific history remains in its existing
Request Snapshot/History authority. Frozen child model authority is labelled
`frozen_child`, not a mutable Session selector.

`SettingsLifetimes` is authored by Rust and installed exactly from the native
snapshot. Unattached TUI state has no lifetime matrix (`null`). `/settings` uses
each category's supplied boundary; its only mapping is enum-to-English rendering.
No model-set or approval response repairs semantic TUI state: only native snapshot
and event reconstruction do that.

A save has two distinct boundaries: native selection capture under the coordinator
lock, then disk publication under the file lock. A later native control can change
live selection while that captured save is pending; no disk/live transaction is
claimed. Existing attachment read-only/detached checks and the native Session
replacement fence run before capture. Historical and frozen-child hosts have no
default-writer authority.

## Configuration write guarantee

The expected revision is SHA-256 of the exact document bytes, including comments
and whitespace. `missing` is the distinct absent-document revision. Saves reject
symlink/nonregular targets and oversized input. The canonical parent and fixed
filename select a persistent sibling lock (`.settings.toml.lock`); rustX writers,
including initialization, acquire that file lock and never unlink its inode.
This serializes cooperating writers across threads/processes on supported local
Linux/macOS filesystems. It is not distributed locking.

After lock acquisition, the writer rereads and checks the expected fingerprint.
The existing `toml_edit` CST changes only the finite selected properties, retaining
unrelated fields and comments. Ambiguous duplicate keys (including escaped key
spellings) are rejected. An unsupported shape is refused rather than replaced with
a freshly serialized whole JSON document. No secret-bearing configuration is
returned in results, diagnostics, or debug formatting.

The writer stages in the target directory, preserves existing permissions, syncs
the staged file, and validates those bytes with the **same canonical TOML parser,
user-layer serde schema and field-ownership checks** used by launch. This proves
user-document validity, not full launch readiness in a mutable project. The writer
retains only the target directory and captured model catalog: it does not retain a
LaunchRequest, reopen project configuration, resolve trust, reload the catalog, or
inspect resource files. Full prospective launch readiness remains the job of
`rustx config check`, `rustx config show --sources`, and normal launch resolution.

A model save changes only `agent.model.model` and `agent.model.reasoning_profile`, but validates
**the complete model object parsed from the staged bytes**, including preserved
`request_params` (the opaque structured TOML object authoring surface),
`max_output_tokens` (the model output policy), and `summary_model` (the summary
model policy). These are members of `agent.model` in `settings.toml`. The canonical
`SessionModelConfig` schema supplies its defaults; `analyze_session_model_config`
uses native `analyze_selection` semantics for the primary selection and any explicit
summary, with their respective Session and summary request-parameter layers. The
same native context-budget rule as launch is applied to the staged user context
(with built-in defaults) and those model limits. Thus preserved settings cannot
silently become invalid after changing model/profile. Approval-only saves retain
the user schema/ownership gate but do not validate unrelated model semantics.

This is user-document semantic validation against the captured catalog, independent
of project precedence, not full current-project prospective launch resolution. A
valid project model cannot hide an invalid staged user selection. Validation is
offline and never resolves credentials or starts provider, process, or Session work.
The writer then rereads the target for a **final fingerprint check**, and only then
renames the candidate over it.

Every failure before publication leaves the previous document authoritative (unless
an external editor itself changed it). A detected conflict is never knowingly
overwritten. A stale client must reread and review before issuing another save;
there is no retry with a guessed new revision. Failed staging is removed by the
temporary-file owner. This is a single-file operation; no multi-file transaction or
atomicity with live runtime state is claimed.

A noncooperating editor can write **after the final check but before rename**. The
rename can replace that edit. This implementation is **not universal filesystem
compare-and-swap**; it detects changes observed before its final check, not every
possible external edit. The final-check hook test deliberately demonstrates the
unexcluded race. The retained revision is a content fingerprint, so an external
A → B → A edit is indistinguishable from unchanged content by design.

## Deterministic regression map

| Test | Boundary/evidence |
| --- | --- |
| `cfg238_save_preserves_comments_unrelated_content_and_redacts_outputs` | Actual published bytes; only selected fields change; safe read/result/debug serialization |
| `cfg238_worker_failure_reports_unknown_publication_without_claiming_rollback` | Worker fails after a successful save; published bytes remain authoritative and error reports unknown outcome |
| `cfg238_stale_revision_rejected_after_publication` | Successful writer B then expected-A save; locked reread rejects |
| `cfg238_competing_writers_serialize_then_recheck_expected_revision` | First writer parked at `Locked`; second announces `BeforeLock`; second lock acquisition observes first publication and rejects old revision |
| `cfg238_external_edit_before_final_check_is_rejected` | Noncooperating edit at `Validated`, before final check; exact external bytes survive |
| `cfg238_external_edit_after_final_check_demonstrates_non_cas_limit` | Edit at `FinalChecked`; rename replaces it, explicitly proving the limitation |
| `cfg238_invalid_candidate_and_staged_failure_preserve_old_document` | Invalid native profile, plus injected errors at `Staged`, `Validated`, `FinalChecked`; no publication |
| `cfg238_duplicate_keys_and_symlinks_are_refused` | Unsupported document/authority forms; bytes retained and diagnostics redacted |
| `cfg238_dogfood_distinct_owners_admission_requests_reload_save_and_reconnect` | Init/check/composition, disk A, admission-gated C/on, later Session B/off, actual scripted provider requests, approval pending, separate save, busy/success/failed reload, detach/attach equality, fresh launch/Session observes saved B |

Issue #256 effective-extension regressions:

| Test | Boundary/evidence |
| --- | --- |
| `ext256_a_live_root_projects_its_frozen_effective_extension_composition` | A live root projects its exact frozen Time/timezone/Background composition and, separately, `agent_status = None`; the composed-status window is provably empty while the extension reports as composed; a fresh prospective resolution of the edited document disagrees with the live projection |
| `ext256_reload_cannot_recompose_extensions_but_the_next_launch_does` | A published resource generation changes neither the composed runtime nor its projection, in both directions; restart projects the new launch's composition and preserves canonical Session history |
| `cfg238_dogfood_distinct_owners_admission_requests_reload_save_and_reconnect` | Live E2E: the launch-authored composition is projected exactly; real Agent Status observations appear without changing it; a successful reload leaves it unchanged; reconnect reconstructs the identical view; the prospective next launch reads the edited document |
| `ext256_a_child_projects_only_its_frozen_extension_composition` | A child's own Runtime Client host projects its frozen `ResolvedSubagentSpec.extensions` while a conflicting root-shaped document sits beside it, including after the spec crosses its real serialization contract; `frozen_child` evidence and the `frozen_admission` extension lifetime |
| `ext256_a_child_frozen_on_r1_keeps_r1_extensions_after_r2_publishes` | The projection of the committed R1 child after a real `reload_resources` publication of R2, and R2's own different projection |
| `ext256_root_extension_configuration_never_reaches_a_named_role` | Root and child projections differ; the root's frozen timezone and disabled Background appear in no child projection |
| `ext256_historical_inspection_reports_no_effective_extension_composition` | Durable-only inspection reports `null` on both the attach and resync read paths, while the inspected history itself carries canonical Agent Status evidence |
| `ext256_effective_extension_protocol_fixture_round_trips_exactly` / `EXT256 shares the native effective-extension protocol fixture exactly` | The Rust and TypeScript halves of `tests/fixtures/runtime-client/settings-v26.json` agree on all three semantic states and both lifetimes |
| `ext256_materialized_owners_recover_the_exact_frozen_composition` | The projection source is the materialization: every semantic composition round-trips through its materialized owners unchanged |
| `EXT256 /settings distinguishes absent, composed, and timezone-configured Agent Status` | Rendering separates absent, composed, contributors-disabled, configured timezone and none, and root from frozen child |
| `EXT256 /settings never infers extension enablement from Agent Status observations` | Enabled with an empty status window renders enabled; absent with a composed status renders absent |

Issue #259 Todo-extension regressions:

| Test | Boundary/evidence |
| --- | --- |
| `ext259_ordinary_tool_selection_neither_adds_nor_removes_the_extension_tool` | Every ordinary activation shape, including `--no-tools` and `--no-builtin-tools`, against both compositions; the extension Tool is present or absent purely by composition, and never appears in the ordinary available catalog |
| `ext259_todo_is_rejected_on_every_ordinary_selection_surface` | Root `agent.tools.builtin`, the CLI activation policy's three lists, and the shared source-qualified selection vocabulary all refuse `todo` by name |
| `ext259_a_workflow_cannot_admit_an_extension_tool` | A Workflow naming `builtin:todo` fails at compile time with the extension named |
| `ext259_todo_is_not_an_ordinary_child_capability` | A frozen Builtin selection naming `todo` fails child materialization closed; the extension plane is the only seam that registers it |
| `ext259_the_todo_tool_schema_is_stable_across_list_mutations` | Create/complete/clear through the real batch authority leave the Tool definition and the capability revision unchanged |
| `ext259_todo_and_agent_status_are_independent_and_change_no_loop_semantics` | All four combinations over one scripted attempt; admission, request count, canonical history, tool identities/statuses, terminal ordering and durable event ordering are identical across all of them |
| `ext259_disabling_todo_preserves_history_and_re_enabling_reconstructs_it` | Three consecutive compositions over one durable store; the disabled launch composes no list and rewrites no history; re-enabling recovers the accepted snapshot with no duplicate ToolResult or event, and continues id allocation |
| `ext259_a_resource_reload_cannot_install_or_remove_the_todo_extension` | Two real capability publications that genuinely change the ordinary plane, in both directions, leave the extension Tool exactly as composed |
| `ext259_parent_and_concurrent_child_todo_lists_never_alias` | A parent and two concurrently composed Todo-enabled children write through their own batch authorities; each list holds only its own task and each allocates id 1 |
| `ext259_a_role_todo_default_and_its_invocation_override_freeze_exactly` | Role default, missing dimension, present dimension, and `extensions: {}` resolve to the exact frozen child set; it survives the child serialization contract and participates in the profile digest |
| `ext259_todo_obeys_the_shared_override_authority_distinction` | A main-model caller holding no Todo is refused by name; an entitled caller and a trusted Workflow override both resolve; enabling or disabling Todo changes no ordinary capability, Skill, or model |
| `ext256_a_child_frozen_on_r1_keeps_r1_extensions_after_r2_publishes` | Extended for #259: R1 composes Todo and R2 removes it; the frozen child keeps R1's Todo composition and a child resolved after R2 composes none |
| `shows no current list for a runtime that composes no Todo extension` | TUI: a Todo-disabled snapshot yields no panel and an explicit `/todos` message, while the inherited `todo` result still renders as transcript history and cannot install a current list |
| `EXT256 reconnect reconstructs the same effective-extension view` | A fresh projection of the same snapshot is identical; an unattached client has no composition |

Repair regressions additionally cover:
- `cfg238_staged_model_preserved_output_budget_is_validated`: a valid A/4096 user
  document cannot become B/4096 when B permits only 2048; rejection after staging
  and before `Validated` preserves exact original bytes and revision.
- `cfg238_staged_model_preserved_request_params_are_validated`: preserved `messages`
  is legal as an opaque Responses parameter but protected for Chat Completions;
  the same pre-publication rejection preserves bytes and revision.
- `cfg238_staged_model_validates_user_context_with_builtin_defaults`: the existing
  context-window/reserve/output rule rejects a newly impossible user budget.
- `cfg238_approval_save_does_not_validate_unrelated_model_semantics`: approval
  remains writable when unrelated model/context semantics are invalid.
- `analyze_session_model_config_validates_primary_and_explicit_summary` and
  `analyze_session_model_config_validates_complete_summary_selection`: native
  primary/summary layers, effective budgets, and explicit summary failures.
- `cfg238_user_write_does_not_reopen_mutable_project_or_catalog`: launch through an
  explicit project document, then delete/corrupt it and remove the catalog file;
  user save still succeeds and never modifies the project.
- `cfg238_project_override_cannot_mask_invalid_saved_selection`: valid project
  selection cannot mask an invalid saved model/profile; validation never reaches
  the `Validated` frontier and both documents retain their prior bytes.
- `cfg238_user_layer_schema_and_ownership_still_gate_publication`: unknown fields,
  forbidden authority, and incorrect nested shapes/types fail before publication.
- The dogfooding test parks the projection publish gate after native B commits and
  its model-set response returns. The client still holds A; target-only save writes
  B before that gate is released. Desired FullAccess is saved while admitted Policy
  still requires approval; subsequent admission uses the reconciled policy.
- TUI scripted-peer coverage delivers the B response without its event, keeps A in
  presentation state, and proves save sends only `target`. A separate test changes
  every supplied native lifetime and checks each rendered section follows it.

Existing native approval, Subagent/Workflow admission and resource publication
suites remain the execution proofs; this feature does not replace those owners or
create a second conformance framework. CI runs the filesystem writer tests on
Linux and macOS; local Linux validation does not imply local macOS execution.

Saved selections do not persist `--models` or a catalog path. A future launch must
use a catalog containing that model/profile. The writer validates against the
captured catalog and the canonical user-document schema; it does not
silently redirect the user's model catalog.

Additional owner evidence retained in the full suites:
`model_update_freezes_at_admission`,
`approval_mode_settlement_reconciliation_precedes_next_admission`,
`reload_changes_only_future_requests_and_preserves_historical_snapshots`,
`reload_publishes_workflow_catalog_and_retains_it_on_candidate_failure`,
`cfg236_gated_frozen_child_retains_r1_after_canonical_role_r2_publication`, and
`an_attempt_frozen_on_r1_resolves_r1_after_r2_becomes_current`.
The CFG238 dogfooding test additionally attempts a real native write after an
in-flight approval-mode change: approval remains required, denial prevents the
write, and the tool continuation still requests C/on before a new admission uses B/off.
