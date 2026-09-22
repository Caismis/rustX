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
never recomputes inheritance (`workspace.foo ?? user.foo`) and never materializes
a native default into an authored draft.

### Two orthogonal dimensions, never one state machine

`unitFacts` reports **authored state** and **effective resolution state**
separately, because they answer different questions:

```text
authored   present | absent | invalid | unavailable   does this exact scope author the unit
effective  available | unset | invalid | unavailable  did native resolution produce a value
```

`authored` reads only `SourceSettings[scope]`. Native sets `authored` and
`diagnostic` exclusively — a document that parses always yields an authored
layer — so `invalid` means "this scope's document did not load", `absent` means
"it loaded and omits this unit", and `unavailable` means the scope has no view at
all. Absent is never `false`, `[]` or `{}`.

`effective` reads `SourceSettings.resolved` and `prospective_diagnostic`. Native
drops `resolved` exactly when a participating document fails to parse, and sets
`prospective_diagnostic` when the merged documents parse but do not resolve. So a
valid Workspace source that authors nothing still reports
`authored=absent, effective=invalid` when the User document is malformed. That is
never rendered as `Unset`, as an empty value or as a native default, and it never
disables authoring of the valid scope: repair, inventory, diagnostics and exact
CAS writes stay available. `unset` is the distinct fact that resolution succeeded
and no source authors the unit, so the native default governs.

### Exact provenance identity

`unitProvenancePath` maps every semantic unit to the exact dotted key native
records, derived from `RuntimeLayer::overlay`: `replace` records a whole-unit
path, `named` records `<container>.<identity>`, and `replace_origin` drops that
path's descendants. So `providers.a`, `models.a`, `agent.tools.sources.a`,
`native_tools.read`, `mcp_tool_policies.a`, `environment.A` and `mcp_servers.a`
each resolve independently; a sibling identity's name, length or insertion order
cannot change the answer. `unitProvenance` then takes the exact key, else the
nearest recorded ancestor (a member omitted from a replaced object belongs to
that winning object), else the unit's own members — reporting `mixed` when they
disagree rather than electing a longest key. `app_server` and named-Agent
resources have no native origin at all and report `unavailable` instead of an
invented claim.

### Owner navigation

`ConfigurationApplication.scope` is an application-scope key — the Session
identity for a Session application — and is never a source owner.
`applicationOwners` reads the native `sources` projection instead, so no code
parses a scope string, guesses from a Session `cwd` or rebuilds source ownership.

Settings surface states (`connecting`/`loading`/`ready`/`stale`/`failed`) and
change behavior (`Applies immediately`/`Requires App Server restart`) are likewise
projected rather than inferred.

## Authoring intent

`UnitForm` keeps five facts distinct and never collapses them into one form value:

```text
native effective value        projected from SourceSettings.resolved
native authored value         this scope's own membership (`authored` prop)
local override intent         exists only after Override or a real edit
local dirty draft             the value carrying that intent
CAS base revision             the exact revision the next write is fenced on
```

For a Workspace unit with no override the control **displays the native effective
value** while authored intent stays absent and the form stays clean. Rendering,
opening and navigating create nothing. `Save` is unavailable until a draft exists,
so a no-op Save can never turn "no Workspace override" into an explicit `[]`,
`{}`, `false`, the inherited value or any other client default. An override is
created only by an explicit `Override <unit>` action or by an unambiguous edit
transition, both of which start from what the control already displays.
`blank` is only the editing seed for a unit this scope has yet to author; it is
never presented as an effective value. `Remove <unit>` sends `authored: null` at
the exact reviewed revision (at Workspace scope that is "use the global default")
and never copies the User value, while an explicitly authored empty array, object
or `false` stays that exact value. `Discard draft` returns to the inherited
presentation rather than to a client-side empty default. A Provider editor
declares itself non-inheritable, because a credential is never read back from a
shadowed definition.

## Composition

`presentation/settings/SettingsRoot` remains the sole modal/navigation owner.
`SettingsContent.module.css` adapts the pinned Harness Models editor and Plugin
field/inventory vocabulary: outlined identity cards, filled editing modules,
compact field rows, disclosures, diagnostics and narrow layouts. `Switch` is the
pinned controlled Harness primitive. `SettingsContent.tsx` contains small
rustX-authored presentation seats, with no protocol imports or persistence.

`app/settings/Settings` reads the existing typed AppServerClient operations. It
composes runtime, catalog, Root, MCP and named-Agent adapters with the resource
inventory. `UnitForm` presents one native mutation's editable intent. The
`SettingsTransactionStore` (created in `app/settings/drafts.tsx`, one per exact
endpoint/authority/target lifetime) owns each unit's draft, exact CAS base,
submitted-operation token and intent generation, committed acknowledgement and
final projection settlement. `Settings` records both the confirmed native commit
and the adopted authoritative projection directly on that store, so a save that
completes after its editor unmounts still retires exactly its own submitted
intent and never a newer draft. Identity includes endpoint, client authority and
the exact Settings target (`user` or `workspace:<id>`), not source revision or
focused Session. Base revision is independent. Workspace A drafts never become
Workspace B or User drafts. No draft, credential, configuration or runtime
snapshot goes into browser storage. Clean forms follow authoritative source
updates; dirty forms require review.

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
`app/settings/drafts.tsx` owns the per-unit editing transaction (draft/context
state, operation identity and its projection settlement); authored membership and
provenance projection live in the pure `app/settings/projection.ts`. `UnitForm`
no longer takes an `initial` value seeded from `authored ?? <client default>`;
call sites pass the exact native `authored` value plus a separate `blank` seed. No
compatibility export, alternate Settings root, legacy mode or feature flag exists.
Tests formerly addressing newline textareas now exercise structured identity rows.
