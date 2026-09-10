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
| Saved primary model/profile or approval default | Rust `UserDefaults` document writer | Explicit `user` scope: `$XDG_CONFIG_HOME/rustx/settings.jsonc` (or `$HOME/.config/rustx/settings.jsonc`) | Same-directory rename publishes one validated document | Only the explicitly selected fields | Does not replace Session model or approval | Can affect a future launch; project/CLI precedence still applies |
| Attempt model, resource revision, approval mode | Native admission | Actual frozen model/resource/policy snapshots | Frozen together at admission; never live-mutated | Retained execution/request evidence only | Retained admitted facts unchanged | No reconstruction from new defaults |
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
- `/model profile <id|default>` reads the whole current native model configuration,
  replaces its reasoning-profile selection, and submits `model_set`. The native
  binding and context validators decide whether the complete selection is valid.
  `default` clears the explicit selection and uses the catalog default profile.
- `/approval policy|full_access` requests the native safe-boundary transition.
- `/show-reasoning on|off` changes rendering only: no provider parameters, model
  selection, tool invocation or canonical history changes. `/reasoning` is unknown;
  there is no compatibility alias.
- `/settings` shows captured sources, Session selection, effective/pending approval,
  actual published resource revision, tools/Skills, and frozen attempt facts.
- `/defaults user` shows only the permitted saved fields, target document and its
  content revision. It does not resolve layers or claim the values win precedence.
- `/save-default user model <revision>` writes exactly `model.model` and
  `model.reasoningProfile` from the displayed Session selection. A cleared profile
  is written as `null`, preserving its nearby comments.
- `/save-default user approval <revision>` writes exactly `approvalMode` from the
  effective runtime mode. Pending desired values are never implicitly saved.

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
authored documents; malformed JSONC cannot be ignored by resource loading.

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

## Configuration write guarantee

The expected revision is SHA-256 of the exact document bytes, including comments
and whitespace. `missing` is the distinct absent-document revision. Saves reject
symlink/nonregular targets and oversized input. The canonical parent and fixed
filename select a persistent sibling lock (`.settings.jsonc.lock`); rustX writers,
including initialization, acquire that file lock and never unlink its inode.
This serializes cooperating writers across threads/processes on supported local
Linux/macOS filesystems. It is not distributed locking.

After lock acquisition, the writer rereads and checks the expected fingerprint.
The existing `jsonc-parser` CST changes only the finite selected properties, retaining
unrelated fields and comments. Ambiguous duplicate keys (including escaped key
spellings) are rejected. An unsupported shape is refused rather than replaced with
a freshly serialized whole JSON document. No secret-bearing configuration is
returned in results, diagnostics, or debug formatting.

The writer stages in the target directory, preserves existing permissions, syncs
the staged file, and validates those bytes through the real launch resolver with
the original document base/ownership rules. Selected model/profile values also pass
native catalog analysis independently of project precedence, so a project override
cannot hide an invalid selection. Validation is offline and never resolves credentials
or starts model/process/Session work. It then rereads the target for a **final
fingerprint check**, and only then renames the candidate over it.

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

Existing native approval, Subagent/Workflow admission and resource publication
suites remain the execution proofs; this feature does not replace those owners or
create a second conformance framework. CI runs the filesystem writer tests on
Linux and macOS; local Linux validation does not imply local macOS execution.

Saved selections do not persist `--models` or a catalog path. A future launch must
use a catalog containing that model/profile. The writer validates against the
captured launch inputs and current prospective document semantics; it does not
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
