# Native Settings and auxiliary presentation (#347)

DeepSeek Harness supplies presentation. Rust owns CFG3 parsing, semantic validation,
overlays, identity, provenance, CAS, serialization, publication and execution.
The browser separates User/Workspace source authoring from Session selection and
adoption. No Session is an authority for source reads, writes or rescans.

## Entry and target ownership

The global Settings entry opens **User Settings** directly against the User source.
The exact Workspace object action opens **Workspace Settings — <workspace>**, bound
to that Product-Host-authorized Workspace for the whole lifetime of the Settings
instance. There is no ordinary `Configuration owner` selector and no second editor
tree: both entries share the same shell and editors. Session focus changes never
retarget an open editor. A Workspace that becomes unregistered or unauthorized, or
whose Product Host/endpoint authority changes, is fenced by the target/authority/
connection epoch and read ordering; its draft and error are retained rather than
redirected to User or another Workspace. Opening either entry allocates no hidden
Session, resident runtime, MCP connection or Python environment.

## Presentation projection

`app/settings/projection.ts` is a pure adapter over the generated protocol. It
projects the authoritative effective value, authored membership and explicit
presence, native provenance/origin, native availability/diagnostics and per-unit
application observations. It never recomputes inheritance (`workspace.foo ?? user.foo`)
and never materializes a native default into an authored draft. Absent override,
`false`, `[]`, `{}`, explicit values, invalid and unavailable are distinct states;
unavailable is not empty, false or a client fallback. `UnitForm` renders the native
effective value plus a provenance-appropriate inherited/default label for a
Workspace with no override, and `Override`/`Use global default` create or remove the
exact native semantic unit through CAS without copying the User value. Settings
surface states (`connecting`/`loading`/`ready`/`stale`/`failed`) and change behavior
(`Applies immediately`/`Requires App Server restart`) are likewise projected rather
than inferred.

## Composition

`presentation/settings/SettingsRoot` remains the sole modal/navigation owner.
`SettingsContent.module.css` adapts the pinned Harness Models editor and Plugin
field/inventory vocabulary: outlined identity cards, filled editing modules,
compact field rows, disclosures, diagnostics and narrow layouts. `Switch` is the
pinned controlled Harness primitive. `SettingsContent.tsx` contains small
rustX-authored presentation seats, with no protocol imports or persistence.

`app/settings/Settings` reads the existing typed AppServerClient operations. It
composes runtime, catalog, Root, MCP and named-Agent adapters with the resource
inventory. `UnitForm` owns one native mutation's editable intent. The target-keyed
`SettingsDrafts` map retains only modified drafts and their exact base revisions
while navigating targets/sections/identities. Identity includes endpoint, client
authority and the exact Settings target (`user` or `workspace:<id>`), not source
revision or focused Session. Base revision is independent. Workspace A drafts never
become Workspace B or User drafts. No draft, credential, configuration or runtime
snapshot goes into browser storage. Clean forms follow authoritative source updates;
dirty forms require review.

Catalog identity suggestions can include published and authored identities. This
is a union of names for input assistance, never a browser overlay or claim of
availability. Provider/Model editors are separate detail views. Model replacement
preserves the generated capabilities, reasoning profiles, request params and
compatibility fields. Named Agents write complete independent profiles, including
delegation fields; Rust validates scope applicability. Description and instructions
remain optional, including when replacing another part of an existing profile.
Native definition construction accepts their schema defaults and enforces their
existing byte bounds; the Web does not invent validation rules. Native source
projections and serialization omit empty text defaults, so an independent profile
edit does not materialize description/instructions in the outgoing mutation.
Root and named-Agent model controls share the native independent explicit Summary
selection (model, reasoning profile, output limit and request params). Changing its
identity preserves the other authored fields; choosing Session deliberately replaces
it with `{ mode: "session" }`. Rendering never materializes native defaults.

## Mutation and recovery

1. Read a native source projection with its revision.
2. Construct a generated `SourceMutation` for one semantic unit.
3. Submit that exact draft base revision. Rust either commits or rejects.
4. Acknowledgement supplies the new authoritative revision and redacted source.
   Successful credential drafts are reconstructed from this projection.
5. Save transfers application responsibility to the native coordinator. Independent
   complete units apply automatically; native context candidates await explicit adoption.

A write acknowledgement is not an application observation. A committed save whose
post-commit authoritative reread fails remains a committed save with uncertain
read/application status: it is never shown as unsaved and never as blanket applied.
Read errors, write errors, CAS conflicts, application failures and unavailable/stale
observations are separate states, and a successful read clears only the read error it
answers. The Advanced section projects per-unit native observations
(Applied/Preparing/Failed/Restart pending) and native change behavior independently;
no single global success state erases a pending adoption or a failed unit.

Conflict preserves the draft and original revision. The user can inspect the
current redacted unit and deliberately choose “Use reviewed revision” before
resubmitting. Remove submits `null`; empty arrays, `all`, exact arrays and empty
objects remain distinct generated values. No recursive merge or config-file
serialization is implemented in TypeScript.

Lost Save/adoption replies and reconnect cause authoritative rereads, never replay.
Connection/target epochs fence obsolete work, and a read sequence prevents an
older overlapping read from replacing a newer observation or a write acknowledgement.
Both success and rejection commit only within the same epoch and read sequence,
including explicit refresh failures.
Busy adoption leaves work running. Failed preparation leaves old effective resources
available. Settings renders native per-unit state without inferring field impact.

## Coverage and native limits

General edits Root identity/description/instructions, approval, context, model
and Tool deadlines, environment and child capacity. App Server process policy is
User-only. Native classification distinguishes hot fields from restart fields;
the source projection retains desired and actual process bindings across reopening.
Reverting desired to actual clears restart state through native authority.

Root model selection, Native/source Tools, Skills, Plugins, named Agents,
Workflows and AGENTS.md guidance use native semantic units. Agent Status
contributor selectors preserve unspecified/default intent; the UI does not
substitute `false` for a native default. Plugins remain the closed Rust set.

Resource cards separate source ownership/shadowing, validity, preparation and
Root selection. MCP transport badges/forms prefer an explicit type, then infer
HTTP from a URL or stdio from a command. This display projection does not add a
type or change retained headers/environment; an explicit transport switch replaces
the transport-shaped draft. Invalid winners do not borrow a shadowed definition. Skills show
native package provenance/diagnostics and explicitly describe prompt visibility,
not filesystem permissions. Python/MCP preparation and Workflow admission status
are native facts. Unprepared definitions are not probed by the browser.

The generated source-write API offers config, MCP and Agent units. It does not
offer Workflow-program, Skill-package or Python-source authoring; these inventories
do not fabricate such operations. This is the supported native contract, not a
parallel editor implementation. Workflow Root selection is editable.

## Native auxiliary views

Todo and Queue retain their native snapshot adapters and existing Harness-derived
docks. Goal consumes #351/#352's sole `GoalPhase` authority, with revision-fenced
controls and no browser lifecycle. The obsolete `Conversation.tsx` modified by
#352 stays deleted; semantic Goal Tool labels are mapped in `bindings/tools`,
while exact Tool names remain diagnostic attributes and native Trace identities.

Subagent and Workflow activity use native snapshot identities/states in shared
presentation cards. Background execution retains the Harness Tool card with native
execution identity, result, uncertainty and progress. Trace retains its existing
native history/cache and virtualized Harness-derived ledger; no UI-observation
history is synthesized.

The existing right-panel seat displays Inspector or a bounded managed-artifact
preview. Artifact bytes come only from `artifact/read` through ArtifactResources;
attachment fences, two transfers, 256 KiB, sixteen object URLs and deterministic
URL cleanup remain enforced. Text is rendered as inert text, never executed HTML.
There is no path browser, filesystem editor, Host resource registry or filesystem
authority. Unsupported binary types keep download presentation.

Appearance persists only a validated light/dark preference in the app owner.
Inspector projects native facts and the existing bounded wire log. Neither is a
second configuration or runtime authority.

## Cleanup

Deleted `app/settings/Settings.module.css`; all Settings adapters share the
presentation sheet. Removed the duplicate global `.activity-card` rules. The
obsolete `app/Conversation.tsx` remains deleted after Goal reconciliation. Removed
the ordinary `Configuration owner` selector and its catalog-driven target switching;
the target is now an explicit immutable Settings prop and drafts are keyed by it.
`app/settings/drafts.tsx` keeps only draft/context state; authored membership and
provenance projection moved to the pure `app/settings/projection.ts`. No
compatibility export, alternate Settings root, legacy mode or feature flag exists.
Tests formerly addressing newline textareas now exercise structured identity rows.
