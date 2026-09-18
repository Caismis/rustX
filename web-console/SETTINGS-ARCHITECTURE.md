# Native Settings and auxiliary presentation (#347)

DeepSeek Harness supplies presentation. Rust owns CFG3 parsing, semantic validation,
overlays, identity, provenance, CAS, serialization, publication and execution.
The browser has exactly Effective, User and Workspace configuration views.

## Composition

`presentation/settings/SettingsRoot` remains the sole modal/navigation owner.
`SettingsContent.module.css` adapts the pinned Harness Models editor and Plugin
field/inventory vocabulary: outlined identity cards, filled editing modules,
compact field rows, disclosures, diagnostics and narrow layouts. `Switch` is the
pinned controlled Harness primitive. `SettingsContent.tsx` contains small
rustX-authored presentation seats, with no protocol imports or persistence.

`app/settings/Settings` reads the existing typed AppServerClient operations. It
composes runtime, catalog, Root, MCP and named-Agent adapters with the resource
inventory. `UnitForm` owns one native mutation's editable intent. The session-keyed
`SettingsDrafts` map retains only modified drafts and their exact base revisions
while navigating scopes/sections/identities. Closing Settings drops this memory.
No draft, credential, configuration or runtime snapshot goes into browser storage.
Clean forms follow authoritative source updates; dirty forms require review.

Catalog identity suggestions can include published and authored identities. This
is a union of names for input assistance, never a browser overlay or claim of
availability. Provider/Model editors are separate detail views. Model replacement
preserves the generated capabilities, reasoning profiles, request params and
compatibility fields. Named Agents write complete independent profiles, including
delegation fields; Rust validates scope applicability. Description and instructions
remain optional, including when replacing another part of an existing profile.
Native definition construction accepts their schema defaults and enforces their
existing byte bounds; the Web does not invent validation rules.
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
5. Save changes source only. The loaded generation stays authoritative until
   explicit Reload prepares and atomically publishes its replacement.

Conflict preserves the draft and original revision. The user can inspect the
current redacted unit and deliberately choose “Use reviewed revision” before
resubmitting. Remove submits `null`; empty arrays, `all`, exact arrays and empty
objects remain distinct generated values. No recursive merge or config-file
serialization is implemented in TypeScript.

Lost Save/Reload replies and reconnect cause authoritative rereads, never replay.
Connection/attachment epochs fence obsolete work, and a read sequence prevents an
older overlapping read from replacing a newer observation or a write acknowledgement.
Both success and rejection commit only within the same epoch and read sequence,
including explicit refresh failures.
A failed/busy Reload leaves the previous generation and native diagnostic visible.

## Coverage and native limits

General edits Root identity/description/instructions, approval, context, model
and Tool deadlines, environment and child capacity. App Server process policy is
User-only and restart-required. Effective shows that authored process intent is
not evidence of a live process policy change.

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
obsolete `app/Conversation.tsx` remains deleted after Goal reconciliation. No
compatibility export, alternate Settings root, legacy mode or feature flag exists.
Tests formerly addressing newline textareas now exercise structured identity rows.
