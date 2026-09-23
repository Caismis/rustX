# Native Settings and auxiliary presentation (#347)

DeepSeek Harness supplies presentation. Rust owns CFG3 parsing, semantic validation,
overlays, identity, provenance, CAS, serialization, publication and execution.
The browser separates User/Workspace source authoring from Session selection and
adoption. No Session is an authority for source reads, writes or rescans.

## Orchestration: explicit actors, not component effects

Configuration UI asynchronous semantics are owned by explicit actors and state
machines (XState v5). React presentation may subscribe and submit intent;
component-local effects, counters, refs or booleans are **not** authoritative for
configuration transaction, read ordering, adoption or terminal settlement.

```text
native App Server / Product Host authority
        ↓
XState v5 actors and machines      app/settings/machines/*
        ↓
explicit machine snapshots + events
        ↓
React presentation                 Settings.tsx, controls.tsx, SessionConfiguration.tsx
```

XState orchestrates browser behaviour only. It is never configuration authority:
every value, provenance, conflict, application and adoption decision stays native.

### Module hierarchy

```text
app/settings/machines/port.ts                  the one native I/O boundary of a Settings target
app/settings/machines/settings-target.ts       the Settings authority of one exact target
app/settings/machines/unit-transaction.ts      one native semantic unit's CAS editing transaction
app/settings/machines/session-configuration.ts Session observation + Session adoption
app/settings/machines/navigation.ts            top-level Settings navigation and owner lookup
app/settings/machines/system.ts                lifetime ownership of the actors above
app/settings/machines/react.ts                 the React subscription boundary
```

Machines never touch a client, a socket or a Product Host: they invoke a
`ConfigurationPort` / `SessionConfigurationPort` / `OwnerLookup`. That is what
lets a machine test drive the real transition graph with deferred promises
instead of timers, and it keeps transports out of machine context.

### Actor ownership and the explicit lifetimes

```text
ConfigurationSystem(client)                    keyed by (endpoint, authority revision)
  ├── settingsTarget: user                     ─┐
  ├── settingsTarget: workspace:<id>            │ one per exact target
  │     └── unitTransaction: <semantic unit>    │ one per unit touched in this lifetime
  └── sessionConfiguration: <session>          ─┘ reference counted by its presentation

React Settings dialog ──ATTACH/DETACH──▶ an existing target actor
```

- **App Server authority lifetime** — `(endpoint, authorityRevision)`. Replacing
  the authority retires every target actor of the old lifetime and starts the
  replacement from nothing, so old state cannot leak into it. A target with no
  native mutation in flight (`mutationInFlight`, the `mutation.submitting` span)
  is stopped and dropped at the replacement itself — an unsaved draft, including
  a Provider credential or a literal environment value, belongs to the authority
  it was authored against and is never retained, migrated or replayed. A target
  whose mutation already crossed the native submission boundary is detached and
  kept only until that mutation leaves `submitting`: a definitive
  acknowledgement or failure must still settle the exact transaction that
  submitted it, under its own old authority. The `ConfigurationSystem` observes
  that settlement through a subscription it owns and stops and drops the actor at
  once — no poll, and no dependence on a later authority replacement.
- **Transaction lifetime** — per-unit actors live for the whole authority
  lifetime. Closing Settings, changing section or switching target is a React
  unmount and cannot reach them.
- **Presentation lifetime** — `ATTACH` / `DETACH`. A detached presentation reads
  nothing and converges nothing; it neither polls nor keeps a background read
  alive. A mutation already in flight still settles. The invariant of the
  boundary is exactly:

  ```text
  DETACH retains editing transactions.
  ATTACH revalidates authoritative observation.
  ```

- **Observation lifetime** — an authoritative observation belongs to exactly
  one presentation attachment and one connection generation. `DETACH` (and
  `ATTACH` itself) demotes it to stale presentation data; a replaced generation
  retires it outright. An observation retained for stale presentation purposes
  is never accepted as fresh authority for the new attachment: every new
  Settings presentation attachment must establish a fresh authoritative
  observation for that exact target and the current connection generation
  before any observation is current again.
- **Connection lifetime** — a new `generation` retires exactly what the
  replaced generation observed — its projection, its read failure and its
  convergence report, all owned by the `authority` region — and nothing else. It
  deliberately keeps the transactions and the mutation outcome: a reconnect is
  not a reason to lose a draft, a pinned CAS base, a definitive commit awaiting
  settlement, or the conflict, rejection or unknown outcome of a save.

### The Settings target machine

Two genuinely independent facts, therefore two parallel regions (plus a small
`maintenance` region for the explicit native rescan):

```text
authority   suspended ──ATTACH──▶ idle ⇄ reading.{current | predatesCommit} → settling → idle | blocked
                                    ↑                                                       ↑
                                    └── awaitingWrite ──────────────────────────────────────┘
mutation    idle → submitting → observing → saved | unobserved
                             ↘ conflicted | rejected | uncertain
```

Important events: `ATTACH`, `DETACH`, `TRANSPORT`, `REFRESH`, `RECONCILE`,
`UNIT.EDIT`, `UNIT.SUBMIT`, `UNIT.REVIEW`, `UNIT.DISCARD`, `UNIT.RETIRED`,
`READ.FORCE`, `READ.ADOPT`, `READ.REREAD_FAILED`, `WRITE.STARTED`,
`COMMIT.PENDING`, `COMMIT.OBSERVED`, `COMMIT.UNOBSERVED`,
`GENERATION.REPLACED`, `TRIGGER`.

Each region owns its own facts, and no region assigns another's:

```text
authority     observation · staleObservation · readError · convergenceError · chasing
mutation      the region state itself · rejection (native detail while `rejected`)
maintenance   maintenanceError
```

The user-visible save status is `mutationOutcome(snapshot)`, projected from the
`mutation` region: `committed` while the post-commit observation is owed,
`saved` (observed or explicitly unobserved), `conflict`, `rejected` or
`uncertain`. Only a new submission leaves one of those states. A connection
generation replacement retires the `authority` facts above and never touches
the `mutation` region, so an outcome reads the same whichever of it and the
replacement arrived first.

`reading` records whether the read in flight postdates every definitive commit
this target has recorded: a read is issued `current`, and a commit recorded
while it is outstanding (`COMMIT.PENDING`) makes it `predatesCommit`. Such a
read is still adopted and still fails into `blocked`, so its outcome and the
acknowledgement converge in either order, but it is not the commit's
post-commit observation: it discharges the commit's observation obligation only
if it already carries the committed revision, and it never tells a transaction
that its commit diverged.

`ATTACH` is the one semantic attachment event and the machine owns all of its
consequences: it demotes the previous observation to stale presentation data
and re-enters `attached`, so the standing "no current projection" obligation
performs exactly one fresh authoritative read through the single read owner —
coalesced with any publication or commit obligation already outstanding. A
reattachment whose Workspace write still holds its reread reservation
(`rereadReservation`) rejoins `awaitingWrite` instead of starting a competing
read, and that write's own reread is the new attachment's fresh authoritative
observation. A write that is merely still pending (`submission`) is not enough:
if a newer read or a connection generation replacement revoked its reservation,
the reattachment reads for itself.

### The per-unit transaction machine

Three orthogonal facts, three regions — never a combination of `draft?` /
`pinned` / `submitting?` flags:

```text
intent     clean ⇄ dirty
base       following ⇄ pinned
mutation   idle → submitting → acknowledged.{awaitingObservation → diverged} → settled
                            ↘ unconfirmed
```

A clean Remove is exactly `intent.clean` + `base.pinned`: delete intent pins the
reviewed revision without manufacturing a fake value draft. A late
acknowledgement of an older intent is exactly `mutation.acknowledged` while
`intent.dirty` already holds a newer generation. `unconfirmed` is a submission
that ended without a definitive commit; which of conflict, rejection or unknown
outcome it was is the target's `mutation` region state.

A definitive commit advances the CAS base to the committed revision before any
authoritative read has observed it. `acknowledged.awaitingObservation` is
exactly "no read issued after the commit has completed", so the held
observation differing from the base is not a source change. An `OBSERVED` event
carries `postCommit`; the first post-commit observation that does not carry the
committed revision moves the transaction to `acknowledged.diverged` — whatever
revision it carries, including the exact pre-save revision restored by an
external writer. The commit stays a definitive fact, the next save stays fenced
on the committed revision until the explicit `UNIT.REVIEW`, and a later
observation of the committed revision still settles it. `requiresReview`
answers the review question from these regions; React renders it and never
reconstructs chronology from revision values.

### The Session configuration machine

```text
observation  idle → loading.{requested | adoptionReread} → ready | unavailable
adoption     idle → submitting → idle | rejected | uncertain
```

Observation and adoption are two regions over two separate context fields
(`readError`, `adoptionError`). Neither region can assign the other's field, so a
successful read structurally cannot clear an adoption rejection, and an adoption
response structurally cannot clear a read failure.

One adoption transaction spans both regions, and the `adoptionInFlight` tag names
that span: `adoption.submitting`, then `observation.loading.adoptionReread` — the
authoritative reread the native response owes. The transaction's terminal point
is that reread settling, or a newer read superseding it. Session actors are
reference counted by their presentations; when the last holder leaves, the
`ConfigurationSystem` stops and removes the actor at once, or — while the
adoption transaction is in flight — subscribes and does so exactly at its
terminal point. A holder attaching before then cancels that subscription and
keeps the actor alive.

## Entry and target ownership

The global Settings entry opens **User Settings** directly against the User source.
The exact Workspace object action opens **Workspace Settings — <workspace>**, bound
to that Product-Host-authorized Workspace for the whole lifetime of the Settings
instance. There is no ordinary `Configuration owner` selector and no second editor
tree: both entries share the same shell and editors. Session focus changes never
retarget an open editor. A Workspace that becomes unregistered or unauthorized, or
whose Product Host/endpoint authority changes, is fenced because the Settings
authority actor is addressed by `(endpoint, authority revision, target)` and a
different address is a different actor; its draft and error are retained rather
than redirected to User or another Workspace. Opening either entry allocates no hidden
Session, resident runtime, MCP connection or Python environment.

## Presentation projection

`app/settings/projection.ts` is a pure adapter over the generated protocol. It
never recomputes inheritance (`workspace.foo ?? user.foo`) and never materializes
a native default into an authored draft.

### Two orthogonal dimensions, never one state machine

`unitFacts` reports **authored state** and **effective resolution state**
separately, because they answer different questions:

```text
authored   present | redacted | absent | invalid | unavailable   does this exact scope author the unit
effective  available | redacted | unset | invalid | unavailable  did native resolution produce a value
```

`redacted` is the fifth, separate fact: native reports that a unit *exists* and
never projects its value, because the value is a secret-bearing literal. It is
not `present`, not `absent` and not `unavailable`, and no branch of the editor can
render or copy a value it does not have.

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

Settings navigation itself is one machine owned by the product shell
(`machines/navigation.ts`), and every user action that changes Settings
navigation is an event on it: open User Settings, open an exact Workspace, open
an owning Workspace, open Connection Settings — including the disconnected
recovery "Show details" gesture — select a section inside the open dialog,
close Settings, and authority replacement. The machine's `section` is the one
owner of the displayed Settings surface: `Settings` receives it and sends
`SELECT`, and holds no section state of its own, so a top-level decision taken
while the dialog stays mounted is exactly what it shows.
Each of those re-enters `idle`, which **stops** the owning-Workspace lookup
actor. Owner lookup (`listWorkspaces`) is asynchronous *preparation*, never
standing authority to commit navigation later, and that is now structural rather
than a comparison: a stale lookup has no completion path at all, so neither its
success nor its failure can overwrite a newer decision, reopen a closed dialog or
publish an obsolete error. A lookup that completes under a replaced App Server
authority answers `retired`, which is neither a navigation decision nor an
error — the lookup has no error channel that could report one. There is exactly
one navigation machine; no surface keeps a private epoch.

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
never presented as an effective value. `Discard draft` returns to the inherited
presentation rather than to a client-side empty default. A Provider editor
declares itself non-inheritable, because a credential is never read back from a
shadowed definition; the inherited definition is still reported, redacted.

Removal is the exact inverse of authoring, not a generic mutation. `Use global
default <unit>` (Workspace) and `Remove <unit>` (User) both send `authored: null`
at the exact reviewed revision and never copy the lower scope's value, and both
exist **only while this scope really authors the unit**: removing an absent unit
has no meaning, so an inherited Workspace unit offers `Override <unit>` instead.
Authored presence is the native projection value the call site passes without a
fallback — only `undefined` means "authors none" — so an explicitly authored `[]`,
`{}`, `false` or `""` counts as an override and keeps its removal action. An
invalid or unobserved authored document proves no override, so it offers none.

## Identity discovery

Enumerating an identity is a native fact, not an authored one. Workspace catalogs
list every Provider and Model identity in `SourceSettings.resolved`, together with
whatever this scope authors for it and the native `provenance` origin, so an
inherited identity is reachable without being retyped and is reported as
inherited rather than as an override. Whole-file resource families (MCP
definitions, named Agent profiles) are shadowed as whole identities, so there is
no value to merge: `prospective_resources.definitions` names the winning scope of
each identity, and an identity owned by User is listed as inherited with an
`Override` action. User authoring is the lowest authored source and inherits from
nothing, so its catalogs list exactly what it authors and never present a
Workspace-owned identity as User-overridable. Opening an inherited identity
authors nothing, and an override always begins from a safe authoring seed — for a
Provider that seed contains no credential at all. The same rule governs every
named semantic-unit container reached from a list — source-tool selections, MCP
invocation policies and environment variables — through one shared
`reachableIdentities` projection.

## Sensitive projection boundary

Native authority owns every secret-bearing authored value, and **no projection
that leaves native authority carries one**. That is a type-level fact, not a rule
each surface has to remember:

```text
Provider credential        CredentialSourceView   kind, and an environment variable name
MCP env / headers          McpView                cleared, plus retained_env / retained_headers
Tool environment           RuntimeLayer.environment  a list of identities, never a map of values
```

`RuntimeLayer` is generic over both secret-bearing members
(`RuntimeLayer<P, E>`): authoring instantiates it with `AuthoredEnvironment`
(`BTreeMap<String, String>`) and the single wire view instantiates it with
`EnvironmentIdentities` (`Vec<String>`). `settings::redact` is the only way to
build the view from the authored document, so adding a secret-bearing member has
exactly one place to be redacted. Consequently `SourceSettings.user`,
`SourceSettings.workspace`, `SourceSettings.resolved`, the Advanced diagnostics
that render them verbatim, and any generic `UnitForm` inheritance path *cannot*
express a literal environment value — the browser has no value to leak.

What the browser still gets is everything it actually needs: the identity exists,
which document authors it, its native provenance (`environment.<name>`) and its
availability. Authoring an override therefore replaces the value outright instead
of reading the lower owner's value back, exactly as a Provider credential already
worked. The environment editor declares `redacted`, so it seeds from `blank`,
renders a `password` field and says plainly that the existing value is never
projected.

## Sensitive authoring lifetime

An authored payload may carry a secret: a Provider literal credential, an MCP
literal environment value or header, a literal Tool environment value. Before
submission it lives in exactly one place, the live editing draft. `UNIT.SUBMIT`
hands the transaction actor only the mutation's `RevisionSelector` — the mutation
family, plus a named resource's identity — which is the whole of what settlement
needs to resolve which native document revision a commit landed in. The
transaction therefore never holds an authored payload at all:

```text
before acknowledgement   live draft value (authored, may be secret)
                         token · intent generation · selector
after acknowledgement    token · intent generation · selector
                         committed revision
```

The `COMMITTED` transition clears the confirmed draft unless the browser intent
has already moved on to a newer one the user is still editing. A confirmed commit whose
post-write authoritative reread failed therefore leaves an unsettled transaction
that carries no secret-bearing payload, across editor unmount, section navigation
and Settings close, until a later authoritative projection carrying its committed
revision settles it — with no replay and no second write.

## Composition

`presentation/settings/SettingsRoot` remains the sole modal/navigation owner.
`SettingsContent.module.css` adapts the pinned Harness Models editor and Plugin
field/inventory vocabulary: outlined identity cards, filled editing modules,
compact field rows, disclosures, diagnostics and narrow layouts. `Switch` is the
pinned controlled Harness primitive. `SettingsContent.tsx` contains small
rustX-authored presentation seats, with no protocol imports or persistence.

`app/settings/Settings` renders one Settings presentation and owns no
asynchronous configuration semantics at all: it attaches to the Settings
authority actor of its exact target, selects presentation state from that
actor's snapshot, and sends user intent. It composes runtime, catalog, Root, MCP
and named-Agent adapters with the resource inventory. `UnitForm` presents one
native mutation's editable intent, reading that unit's transaction actor and
sending `UNIT.EDIT` / `UNIT.SUBMIT` / `UNIT.REVIEW` / `UNIT.DISCARD`; it holds no
ref, no effect and no local orchestration state. The unit transaction actor owns
the draft, the exact CAS base, the submitted mutation's revision selector, token
and intent generation, the committed acknowledgement and the final projection
settlement. The target actor records both the confirmed native commit and the
adopted authoritative projection, so a save that completes after its editor
unmounts still retires exactly its own submitted intent and never a newer draft.
Actor identity is endpoint, client authority and the exact Settings target
(`user` or `workspace:<id>`), not source revision or focused Session. Base
revision is independent. Workspace A drafts never become Workspace B or User
drafts. No draft, credential, configuration or runtime
snapshot goes into browser storage. Clean forms follow authoritative source
updates; dirty forms require review.

Catalog identity suggestions can include published and authored identities. This
is a union of names for input assistance, never a browser overlay or claim of
availability. Adding a new identity is offered only for one the catalog does not
already reach, so an inherited identity is overridden rather than re-created. Provider/Model editors are separate detail views. Model replacement
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

**Authoritative read ordering is structural.** The `authority` region is the one
owner of every authoritative source read of a Settings lifetime — startup read,
explicit refresh, convergence read, post-commit read and Workspace write-owned
reread alike — and it is also the single convergence owner. Exactly one read is
in flight; starting a newer read re-enters `reading`, which **stops** the older
read actor. An older read therefore has no completion path at all: it cannot
publish a projection, cannot publish a read failure, and cannot discharge a
commit observation, whatever order the network answers in. There is no
reservation counter to compare and no accepted-projection comparison to get
wrong.

The reread a Workspace write owns is the one read the browser does not start
itself, so it is reserved by a state: `WRITE.STARTED` moves the region to
`awaitingWrite`, which is exactly the old "reserve the read order at write
initiation". A native publication newer than the reservation's watermark leaves
that state for the publication-owned read, and the Host's reread is then silently superseded however late it
arrives — in both outcomes, so a superseded reread failure is not published as
this presentation's read failure either. The commit's own observation obligation
never ejects `awaitingWrite`: that obligation is precisely what the reread is
about to answer — and neither does a reattachment's validation read: an
unrevoked reservation survives the presentation bounce, so `ATTACH` while it
stands rejoins `awaitingWrite` and the write's own reread becomes the fresh
authoritative observation of the new attachment.

The reservation is therefore explicit context, `rereadReservation`, named by the
token of the submission that took it, and it is a different fact from that
submission. It also records the publication watermark it was established
against — the application scope of the observation it was submitted over, and
the version then published for it. The Host's reread is issued after the commit,
so it answers every publication up to that watermark. Supersession compares the
current publication with that watermark only, never with the current
presentation observation, which every `ATTACH` demotes: a newer publication
arriving after a detach/reattach — or while detached — still supersedes the
reservation. The write transaction may outlive its connection generation and any
number of attachments, and still settles exactly once. The reservation is
publication authority and is revoked — never restored — by the first of: entry
to `reading` (a newer read owns the order), a connection generation replacement
(whether or not a presentation is attached), or the write ending. `DETACH` alone
neither takes nor revokes it, and only a new submission ever takes one. The
write's reread is published, in either outcome, only while its own submission
still holds the reservation; otherwise it publishes neither a projection nor a
read failure, and the commit is classified by `COMMIT.PENDING` against whatever
read the current generation owns.

Convergence is level-triggered and has no loop, worker, timer or poll:
`idle` takes an eventless transition to `reading` whenever an obligation is
outstanding — a publication this projection has not reached, a definitive commit
no post-commit read has observed, or no projection at all. Publications arriving
while a read is in flight coalesce, because the region is not in `idle`. After a
read is adopted, `settling` decides once: the obligation is satisfied, or it
advanced (one more bounded read), or native is measurably stale (reported once,
then `blocked`). A failed read also lands in `blocked`, where nothing retries on
its own until a publication, reconnect, reattach or explicit refresh arrives.

The saved notice is the `mutation` region's `saved` / `unobserved` state: it is
reported once the post-commit observation settles — carried by an
authoritative read, or explicitly unavailable — so it is truthful against the
projection the presentation shows. The commit itself was definitive at the
acknowledgement; the projection, the read outcome and the native application
remain separate facts reported separately.

**Presentation lifetime is not mutation lifetime.** `DETACH` retains editing
transactions — dirty drafts, clean Remove intent, pinned CAS bases and submitted
mutation state — while suspending reading, convergence and every presentation
fact. It does not fence a definitive native
`sourceWrite` acknowledgement: the write is invoked by the target actor, not by
the dialog, so it completes and is recorded on the transaction that submitted it
even when the editor, or the whole Settings dialog, unmounted first. A
presentation lifetime can stop an old operation from updating the current UI, but
it never reinterprets a definitive commit as a failed submission — which would
strand an already committed secret-bearing draft and a stale CAS base. Actors are
keyed by endpoint, authority and target identity, so an acknowledgement settles
exactly its own transaction and never reaches a replacement authority or a
different target. An outcome that is genuinely
uncertain keeps the existing authoritative-reread-only recovery, with no replay. Session adoption is one region of the Session
configuration machine, so it leaves `submitting` on the adoption response alone:
the authoritative reread it then raises is settled by the observation region, and
neither a failed reread nor a superseded lifetime can strand the adoption guard. `busy` is
that region's state, never authority — `session/adoptConfiguration` revalidates
its own gate natively.
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
Deleted `app/settings/drafts.tsx` and its hand-written `SettingsTransactionStore`:
the per-unit editing transaction is now `machines/unit-transaction.ts`, and the
only thing left of that module is the `SourceContext` in
`app/settings/source-context.ts`. The `SaveSource` callback prop is gone from
every editor; editors submit intent to the Settings authority actor instead.
Deleted the Settings `epoch` / `reads` / `writing` / `outstanding` / `waiting` /
`commits` / `observedCommits` / `converging` / `publications` / `observation`
refs, the `awaitSettlement` / `wake` / `owns` / `converge` helpers and the
lifetime `useEffect`; the `settingsNavigation` epoch in `App.tsx`; and the
`epoch` / `reads` / `submitting` refs in `SessionConfiguration.tsx`. None of them
has a replacement in machine context: the semantics they encoded are states,
regions and actor lifetimes now. Authored membership and provenance projection
stay in the pure `app/settings/projection.ts`. `UnitForm` takes the exact native
`authored` value plus a separate `blank` seed, and a `redacted` flag for a unit
whose value native never projects. No compatibility export, alternate Settings
root, legacy mode or feature flag exists.
Tests formerly addressing newline textareas now exercise structured identity rows.
