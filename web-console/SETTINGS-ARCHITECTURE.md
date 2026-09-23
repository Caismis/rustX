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
React presentation                 Settings.tsx, <page>/*.tsx, forms/*, SessionConfiguration.tsx
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

client publication ──(authority, TRANSPORT)──▶ every live actor of the current lifetime
                      └─ Settings target: port.publication(configuration) → its own scope only
React Settings dialog ──ATTACH/DETACH──▶ an existing target actor
React Session region  ──retain/release, ADOPT──▶ its Session actor
```

The `ConfigurationSystem` is also the one owner of transport delivery. At every
client publication it first retires a replaced authority, then tells every live
actor of the current lifetime the transport — connection state, connection
generation and, for a Settings target, the publication of its own source
scope; for a Session actor, that Session's publication and authoritative
snapshot — whether or not a presentation is attached or holds it. Retired
actors of a replaced authority are told nothing. No React effect forwards
transport, so no read, reconnect recovery or convergence ever depends on a
component rendering; each machine ignores a delivery that changes nothing it
observes.

**Publication ownership is target-local.** The client mirrors every native
application scope — each Session, each source — in one `configuration` map and
replaces that map on any publication, so its identity is not an observation
fact. A Settings target never sees the map: its `ConfigurationPort` projects it
to the one `ConfigurationApplication` of the exact native source scope that
target owns, compared by scope and version. User is the native constant
`source:user`. A Workspace scope names the exact canonical configuration
directory, which only the Product Host's registration resolution knows: it
names the `SourceTarget` of every Workspace configuration operation. A
successful Workspace read or write is therefore self-identifying — native
answers it with a `SourceSettings` whose `target` is the exact canonical
`SourceTarget` it performed the I/O on, and native publishes that source under
exactly that target's scope — so the port names its scope from the result
itself, before the projection reaches the actor. A projection is never adopted
as authoritative while the port cannot name the publication that owns it, and
no separate naming request has to succeed first. The port also asks the Host's
registration resolution (`resolveWorkspace`) alongside each read while the
scope is unnamed, for one case only: a failed read then still knows which
publication may retry it. The system delivers a newly named level at once.
Until something names it no publication is attributed to that target — the
directory is never guessed from an id, a display name, a Session or a path. A
Session
publication, another Workspace's or the other source's publication therefore
cannot retry, refresh, unblock or advance a target's source reads.

- **App Server authority lifetime** — `(endpoint, authorityRevision)`. The
  `ConfigurationSystem` subscribes to its client and observes this key itself,
  so the client publication that changes it is the retirement linearization
  point: the old lifetime is retired as a direct consequence of that
  transition, never by a later `settingsTarget()` / `sessionConfiguration()`
  lookup, and no actor of it survives into the replacement to start new work
  through the live client. A connection generation change inside one authority
  is not a lifetime change and retires nothing here. Replacing the authority
  retires every actor of the old lifetime and starts the replacement from
  nothing, so old state cannot leak into it. A target with no
  native mutation in flight (`mutationInFlight`, the `mutation.submitting` span)
  is stopped and dropped at the replacement itself — an unsaved draft, including
  a Provider credential or a literal environment value, belongs to the authority
  it was authored against and is never retained, migrated or replayed. A target
  whose mutation already crossed the native submission boundary is detached and
  kept only until that mutation leaves `submitting`: a definitive
  acknowledgement or failure must still settle the exact transaction that
  submitted it, under its own old authority. The `ConfigurationSystem` observes
  that settlement through a subscription it owns and stops and drops the actor at
  once — no poll, and no dependence on a later authority replacement. A retired
  target is suspended, so settling issues no read of either authority. Every
  Session actor of the old lifetime is stopped at the replacement, an adoption
  in flight included: its late response has no completion path, so it can
  neither reread nor replay anything through the replacement.
- **Transaction lifetime** — per-unit actors live for the whole authority
  lifetime. Closing Settings, changing page or switching target is a React
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
            (UNIT.SUBMIT only from idle | saved | conflicted | rejected | uncertain,
             and only while admitsSourceMutation holds)
```

Important events: `ATTACH`, `DETACH`, `TRANSPORT`, `REFRESH`, `RECONCILE`,
`UNIT.EDIT`, `UNIT.SUBMIT`, `UNIT.REVIEW`, `UNIT.DISCARD`, `UNIT.RETIRED`,
`READ.FORCE`, `READ.ADOPT`, `READ.REREAD_FAILED`, `WRITE.STARTED`,
`COMMIT.PENDING`, `COMMIT.OBSERVED`, `COMMIT.UNOBSERVED`,
`GENERATION.REPLACED`, `TRIGGER`.

Each region owns its own facts, and no region assigns another's:

```text
authority     observation · staleObservation · readError · convergenceError · chasing
              · unobservedSettlement (discharged by adoption)
mutation      the region state itself · submission · rejection (native detail while `rejected`)
maintenance   maintenanceError
```

The user-visible save status is `mutationOutcome(snapshot)`, projected from the
`mutation` region: `committed` while the post-commit observation is owed,
`saved` (observed or explicitly unobserved), `conflict`, `rejected` or
`uncertain`. Only a new submission leaves one of those states. A connection
generation replacement retires the `authority` facts above and never touches
the `mutation` region, so an outcome reads the same whichever of it and the
replacement arrived first.

**Source mutation admission is target-wide and has one owner.** Every semantic
unit submits through the one `mutation` region, and `admitsSourceMutation` —
exported from the machine module and used by both the `UNIT.SUBMIT` guard and
the unit forms — is the only admission fact: connected, a current authoritative
observation with no read failure, no `submission` natively in flight, and no
`unobservedSettlement`. Every settlement — definitive commit, conflict,
rejection or unknown outcome — records `unobservedSettlement`, and only the
adoption of an authoritative read issued after it (or, for a commit, one that
already carries the committed revision) discharges it; a failed settlement
forces that read at once, which stops any read issued before it. So while one
unit's mutation is submitting, or its settlement's authoritative observation is
still owed — including when that observation failed — no other unit can submit
against the projection known to predate it. A refused `UNIT.SUBMIT` has no
transition and changes nothing: drafts stay independent, memory-only and
editable, and nothing queues, replays or retries them. Once the post-settlement
observation is adopted, the next mutation is fenced on its unit's own CAS base,
which a moved source still requires the explicit review gesture to advance.

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
lifetime   live → retired (terminal)
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

A semantic unit whose definitive commit still awaits authoritative observation
is not a valid source for a new same-unit draft. Between the acknowledgement and
that observation the base already names the committed revision while the
presented value is still the pre-commit one, so a draft begun there would carry
an old value on a new revision, and exact CAS could not stop it from restoring
what the commit replaced. The transaction's `EDIT` transitions are therefore
guarded on `not(acknowledged.awaitingObservation)`: an edit in that window is
refused by the actor itself, whoever sends it, and `awaitingCommitObservation`
lets the editor disable its fields, Author and Override for exactly that
state. A newer intent authored before the acknowledgement survives it
untouched; editing resumes once an authoritative observation either settles the
commit — a retired transaction's next edit starts fresh from the observed
source and revision — or reveals `diverged`, which keeps its review semantics.
Other units stay editable throughout, behind the target-wide submission
barrier.

#### Discarding browser intent

`DISCARD` abandons browser authoring intent and nothing else. What is
discardable is exactly what `discardable` answers:

```text
intent.dirty                                  the value draft
base.pinned outside mutation.acknowledged     a reviewed revision pinned as intent:
                                              a clean Remove, or a draft's base
                                              after a conflict, rejection or
                                              unknown outcome
```

A definitive commit is not a draft. While `mutation.acknowledged` holds, the
pinned base is the committed revision, and the transaction still owes that
commit's post-commit observation (`awaitingObservation`) or carries the
divergence that observation revealed (`diverged`). `DISCARD` drops a newer
value draft authored over it, but the committed revision, the owed observation
and the review requirement all survive: a discard during `awaitingObservation`
followed by a post-commit read of the pre-save revision still ends
`acknowledged.diverged` with `requiresReview`. While a mutation is
`submitting`, the intent belongs to that mutation and the gesture is not
accepted — the editor is disabled then as well.

#### Retirement

Retirement is derived, not triggered. A transaction owns something while it
holds browser intent (`intent.dirty`), a pinned CAS base (`base.pinned`) or a
native mutation obligation (`mutation.submitting` or `mutation.acknowledged`).
Two events can release the last of these: a discard, and the authoritative
observation that settles a definitive commit — the one carrying exactly the
committed revision. Every region answers that event in one microstep: the
`mutation` region settles, and a base pinned by that commit follows native
authority unless a newer value draft was authored over it. The releasing
transition then raises the internal `RELEASE`, which XState processes as the
next microstep, after all regions have answered. Only `RELEASE` evaluates
ownership, and only from `lifetime.live`; when the transaction owns nothing it
enters the terminal `lifetime.retired`, whose entry announces `UNIT.RETIRED`
exactly once, and the target stops the actor and removes it from `units`.

```text
edit A@r1 → submit → COMMITTED r2 → OBSERVED r2
  intent clean · base following@r2 · mutation settled → RELEASE → retired
edit A1 → submit → edit A2 → COMMITTED r2 → OBSERVED r2
  intent dirty (A2) · base pinned@r2 · mutation settled → RELEASE → live
```

A transaction therefore lives exactly as long as it owns something, and touched
units never accumulate over a long-lived target: editing a retired unit again
starts a fresh transaction from the revision the editor presents. No owner
scans or collects children. The saved notice of a retired unit's commit is the
target's own fact — `committedUnit` answers it from the `mutation` region's
outcome, which only a new submission replaces.

`UnitForm` offers `Discard draft` only while `discardable` holds, so no gesture
named for a draft is ever presented against a commit.

The target's `mutation` region answers the same gesture so that the outcome it
presents and the transaction it describes stay mutually truthful. A conflict
or a native rejection is a definitive non-commit whose only remaining subject
is the preserved draft and base; discarding the intent of the unit it is about
(`outcomeUnit`) retires that outcome to `idle` in the same gesture, so "Your
draft and base revision are preserved" is never shown for a draft that is gone.
An unknown outcome is a fact about native — the write may have committed — so
discarding the intent never turns `uncertain` into a definite non-commit; it
remains until a new submission.

### The Session configuration machine

```text
observation  offline ⇄ connected.{loading.{owed | adoptionReread} → ready | failed}
adoption     idle → submitting → idle | rejected | uncertain
```

The observation region is the transport/read obligation, expressed as states:

```text
offline                     the transport cannot read: nothing reads, nothing polls,
                            nothing is authoritative; triggers are absorbed
connected.loading           exactly one authoritative read in flight
connected.ready             the current connected span's authoritative observation
connected.failed            a read of this connected span failed; the next trigger retries
context.staleApplication    an ended span's observation, stale presentation data only
```

A connected span is one connected stretch of one connection generation.
`connected` is only ever entered from `offline`, which holds no current
observation, so entering it *is* the one read that span owes — its initial state
is `loading`. The invariants are exactly:

```text
A new connection generation never reads until it is connected.
Once connected, an unobserved generation owes exactly one authoritative
Session-configuration read, independent of React or Session attachment.
```

Replacing the generation (or losing the ability to read) ends the span at that
transition: its observation becomes stale presentation data, its read failure is
cleared, and its read in flight is stopped, so an old-generation reply can
publish neither an application nor a read failure. Inside one connected span a
native publication for the Session, a Session snapshot change or an explicit
`REFRESH` re-enters `loading`, which stops the read in flight: one read owner,
and a superseded read has no completion path. A trigger delivered while
`offline` — or together with the transition into `connected` — is answered by
the span's owed read, never by a second one. Nothing polls.

The span-ending transition is deliberately not `reenter`: a re-entering
transition from a region to its own descendant takes the machine root as its
domain and would re-enter the `adoption` region too, resetting an adoption in
flight.

An adoption is submitted on one connection generation. That generation's
replacement ends it as an unknown outcome at once — its reply can no longer
arrive on the replaced connection — and stops the invoked request, so no late
reply of the old connection can settle it. The new span's owed read is the
authoritative reread it needs; the adoption is never replayed. `ADOPT` is
accepted only against the current span's authoritative observation;
`session/adoptConfiguration` still revalidates it natively.

Observation and adoption are two regions over two separate context fields
(`readError`, `adoptionError`). Neither region can assign the other's field, so a
successful read structurally cannot clear an adoption rejection, and an adoption
response structurally cannot clear a read failure.

One adoption transaction spans both regions, and the `adoptionInFlight` tag names
that span: `adoption.submitting`, then `observation.connected.loading.adoptionReread` —
the authoritative reread the native response owes. The transaction's terminal
point is that reread settling, a newer read superseding it, or its connected span
ending. Session actors are
reference counted by their presentations; when the last holder leaves, the
`ConfigurationSystem` stops and removes the actor at once, or — while the
adoption transaction is in flight — subscribes and does so exactly at its
terminal point. A holder attaching before then cancels that subscription and
keeps the actor alive. Authority replacement overrides both: it stops every
Session actor of the old lifetime at once, held or not, adoption in flight or
not.

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

### Malformed documents admit exactly their native recovery

Native parses a document before applying any structured mutation to it, so a
document that does not parse admits none. `documentAuthoring` decides this once
per native document — `structured`, `malformed` or `unavailable` — and
`Settings` decides it once for every page that authors `rustx.toml` units, so no
individual editor rediscovers it:

```text
malformed rustx.toml
  config-backed pages (Models, Agent, Tools & Permissions, Advanced editors)
      → no structured editor, no add/override/remove action
      → "Repair malformed source" (`repair_config`), fenced on the exact revision
  Extensions → Native extensions and each resource's root availability
      → closed on the same terms (they are rustx.toml units)
  Advanced diagnostics and rescan
      → always available: a document that does not parse is when they matter
  independent documents (MCP, named Agent resources, resource inventories)
      → governed by their own native state, unaffected
```

Structured editing returns only after the committed repair is observed by an
authoritative read that parses. The rule is per document, never global: a
malformed MCP document likewise offers no MCP editor — native parses it before
every MCP mutation and has no MCP repair mutation — while `rustx.toml` editing
stays available. A named Agent resource is replaced whole without parsing the
previous file, so an invalid one stays editable.

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
recovery "Show details" gesture — select a page inside the open dialog, focus
a detail inside that page, close Settings, and authority replacement. The
machine's `target`, `page` and per-page `focus` are the one owner of the
displayed Settings surface: `Settings` takes the navigation actor itself, renders
its state as it is and sends `SELECT` / `FOCUS` / `CLOSE`, and holds no page
state of its own, so a top-level decision taken while the dialog stays mounted is
exactly what it shows. The unit-test Settings surface composes the same machine
and a real `ConnectionController`, so no test can reach or hide a navigation state
the product could not.

The machine also owns **target capability**: a Settings target can never enter a
navigation state it does not authorize. `settingsPages(target)` and
`admitsFocus(target, page, focus)` are the whole capability matrix — Models
focuses a Provider or Model, Extensions an extension resource, Advanced
Connection only for the User target, and General, Agent and Tools & Permissions
nothing. `PageFocus` types each focus by the page that owns it. `SELECT` and
`FOCUS` are guarded by exactly those functions, so an illegal request is refused,
never admitted and repaired by the renderer; every transition that changes the
target lands on that target's landing page with no focus. Whether Advanced shows
the Connection entry is that same capability, never the presence of a
`ConnectionController` — `App` supplies one for every target, and Workspace
Settings still cannot expose or enter Connection. Focus is kept per page, so leaving a
Provider detail for Extensions and returning restores that Provider; a new
target starts with no focus, and Connection (`{ kind: 'connection' }` on
Advanced) retargets to the global client without carrying a Workspace's focus.
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

`UnitForm` keeps these facts distinct and never collapses them into one form value:

```text
native effective value        projected from SourceSettings.resolved
native authored presence      whether this scope's source holds the unit (`authoredPresent`)
native authored value         what native parsed it into (`authored` prop)
shadowed definition           the same-name User resource a Workspace one replaces
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
presentation rather than to a client-side empty default, and is offered only
while browser intent exists (see *Discarding browser intent*). A Provider editor
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

For a whole-file resource presence and value are separate native facts, because
presence is a file fact and the value is a parsing fact. A named-Agent file that
exists but does not parse projects its path, real revision and diagnostic with
no authored value; it is still this scope's definition, never `new` and never
`inherited`. It is displayed from the neutral seed rather than from the User
definition it shadows, replaced by an ordinary edit fenced on its own revision,
and removed through that revision — as **Use global default** when a User
definition lies underneath, otherwise as **Remove**. An MCP identity needs no
separate presence: its presence is a fact of the parsed `mcp.toml`, and a
document that does not parse admits no MCP editing at all.

## Identity discovery

Enumerating an identity is a native fact, not an authored one. One rule,
`catalogIdentities`, serves every surface that lists or selects a catalog
identity — the Providers & Models catalog, the Root model selector and a named
Agent's explicit-model selector — so they can never disagree:

```text
User       identities = exactly the User-authored identities
Workspace  identities = the Workspace-authored identities
                        ∪ the native effective identities, when resolution produced them
```

Authored and effective facts are orthogonal: a resolution failure (for example a
malformed User document) removes the effective identities and nothing else, so a
valid Workspace's own authored identities stay reachable. No other scope's
authored layer ever stands in for a missing effective layer. Workspace catalogs
list every Provider and Model identity in `SourceSettings.resolved`, together with
whatever this scope authors for it and the native `provenance` origin, so an
inherited identity is reachable without being retyped and is reported as
inherited rather than as an override. Whole-file resource families (MCP
definitions, named Agent profiles) are shadowed as whole identities, so there is
no value to merge: `prospective_resources.definitions` names the winning scope of
each identity, and an identity owned by User is listed as inherited. User
authoring is the lowest authored source and inherits from nothing, so its catalogs
list exactly what it authors and never present a Workspace-owned identity as
User-overridable. Opening an inherited identity authors nothing, and an override
always begins from a safe authoring seed — for a Provider that seed contains no
credential at all.

The bridge models the two inheritance forms explicitly (`unitOwnership`). A
`value` unit — a `rustx.toml` semantic unit — keeps #391's contract: the inherited
value is displayed and an edit of it is an override of exactly that unit. An
`identity` unit — an MCP definition or named Agent profile — has a definition
lifecycle, `DefinitionAuthoring`:

```text
new         nothing authored or inherited; creation opened it; fields writable
inherited   a User definition is in effect (User document or native inventory);
            inspected read-only, no draft, edits refused by the bridge
overriding  explicit "Override … in this Workspace" copied the secret-free seed
            (`shadowedDefinition`) into the unit's XState draft; nothing written
authored    this scope authors it; removal is "Use global default" when it
            shadows a User definition, otherwise "Remove"
```

Viewing is never authoring: the only transition out of `inherited` is the
explicit override, which is an ordinary `UNIT.EDIT` into the existing unit
transaction, so there is still exactly one draft owner. An MCP override seed
carries the inherited definition's shape and `$VARIABLE` references only — literal
`env`/`headers` are dropped and nothing is retained, because retained keys can
only name values the Workspace document holds; the withheld key names are shown.
Definition and root availability remain two independent transactions. The same rule governs every
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

`presentation/settings/SettingsRoot` remains the sole modal/navigation owner. Its
overlay, focus containment and restoration and its vertical page tabs are React
Aria Components (`ModalOverlay`/`Modal`/`Dialog`/`Tabs`) styled by the retained
Harness classes; the hand-written focus trap and portal it replaced are gone. It
also owns the narrow section menu (see *Presentation (#393)*).
`SettingsContent.module.css` adapts the pinned Harness Models editor and Plugin
field/inventory vocabulary: outlined identity cards, filled editing modules,
compact field rows, disclosures, diagnostics and narrow layouts. `Switch` is the
pinned controlled Harness primitive. `SettingsContent.tsx` contains small
rustX-authored presentation seats, with no protocol imports or persistence.

`app/settings/Settings` renders one Settings presentation and owns no
asynchronous configuration semantics at all: it attaches to the Settings
authority actor of its exact target, selects presentation state from that
actor's snapshot, and sends user intent. It composes the six product pages
(`general/`, `models/`, `agent/`, `tools/`, `extensions/`, `advanced/`).
`forms/bridge.tsx` holds `useUnitEditing`, `UnitShell`, `UnitForm` and
`TypedUnitForm`. `UnitForm` presents one
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
against — the version then published for this target's own source scope. The
Host's reread is issued after the commit,
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
outstanding — a publication of its own scope this projection has not reached,
a settled mutation no post-settlement read has observed, or no projection at
all. Publications arriving
while a read is in flight coalesce, because the region is not in `idle`. After a
read is adopted, `settling` decides once: the obligation is satisfied, or it
advanced (one more bounded read), or native is measurably stale (reported once,
then `blocked`). A failed read also lands in `blocked`, where nothing retries on
its own until a publication of this target's own source scope, a reconnect, a
reattach or an explicit refresh arrives.

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

Agent edits Root identity, description, instructions and project guidance.
Tools & Permissions edits approval, Native Tools, Tool-source selection, Skill
visibility, the delegation allowlists and per-Tool/MCP policies. Advanced edits
context, model and Tool deadlines, environment and child capacity. App Server
process policy is User-only. Native classification distinguishes hot fields from restart fields;
the source projection retains desired and actual process bindings across reopening.
Reverting desired to actual clears restart state through native authority.

Root model selection, Native/source Tools, Skills, native extensions, named
Agents, Workflows and AGENTS.md guidance use native semantic units. Agent Status
contributor selectors preserve unspecified/default intent; the UI does not
substitute `false` for a native default. The native extensions (Todo, Goal,
Agent Status) remain the closed Rust set; there is no plugin runtime,
installation flow or marketplace.

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

#392 deleted `CatalogEditor.tsx`, `RootEditor.tsx`, `RuntimeEditor.tsx`,
`AgentEditor.tsx`, `Integrations.tsx`, `ResourceInventory.tsx`,
`RequestPolicy.tsx` and `controls.tsx`, the Overview page and every
native-unit section route (Providers & Models, Default model, Tool Policies,
Tools, Skill access, Plugins, Agents & Workflows, MCP, Managed Python, Skills,
Workflows, Appearance, Connection, Server & source diagnostics). Their editors
moved into the six pages; no route alias or dual renderer remains.

## Product pages (#392)

```text
Settings.tsx               header (target, lifecycle, Reload configuration) + one page
general/GeneralPage.tsx    client-owned Appearance
models/ModelsPage.tsx      default model; Provider list → Provider detail → Model detail
agent/AgentPage.tsx        Root identity, description, instructions, project guidance
tools/ToolsPage.tsx        approval, Native Tools, sources, Skills, allowlists, policies
extensions/                inventory.ts (facts), ExtensionsPage (list/filter/search),
                           ExtensionDetail (definition + root availability), NativeExtensions
advanced/AdvancedPage.tsx  runtime limits, environment, process policy, diagnostics, rescan
capability.ts              the one resource capability matrix
primitives/aria.tsx        bounded React Aria wrappers used several times across pages
forms/bridge.tsx           useUnitEditing, UnitShell, UnitForm, TypedUnitForm
forms/fields.tsx           typed field controls for TypedUnitForm
forms/controls.tsx         plain value controls (identity rows, selections, checklists)
```

**Capability matrix.** `capability.ts` records, per resource family, whether
native offers an inventory, which source mutation authors a definition
(`mcp`, `agent` or none), which root-selection unit governs it (`source_tools`,
`skills`, `agents`, `workflows` or none), and whether native reports preparation.
Skills, Workflows and Managed Python have no authoring mutation, so no Add,
Edit, Save or Delete is rendered for their definitions; their root selection is
edited through its real unit.

**Definition and selection are independent.** A resource detail renders its
definition form and its root-availability form as two units with two
transactions and two outcomes. The availability form is the *same* unit (same
transaction identity) as the Tools & Permissions control, so there is one draft
and one CAS base. Saving one never submits the other; the target's single
outcome slot reports whichever submission happened last, and a rejected or
uncertain definition stays dirty and unsaved after a selection save.

**TanStack Form is field mechanics only.** `TypedUnitForm` creates a form whose
default values are the unit's displayed value (the actor-owned draft when one
exists). A form-level change listener reflects every value into `UNIT.EDIT`; the
form resets only when the displayed value differs from what it last reflected,
so an authoritative revision update never discards a dirty form, and Save is
`UNIT.SUBMIT`. There is no second draft and no schema mirror: syntactic checks
(required, URL, positive) are local conveniences and native validation decides.
TanStack Form core's devtools event client is aliased in `vite.config.ts` to
`forms/inert-devtools-event-client.ts`, because the stock client broadcasts and
queues complete form state (values included) on `window`; the production build
(`scripts/provenance.ts --artifact`) fails if `tanstack-connect` appears in the
bundle, and a unit test answers the devtools handshake and asserts nothing is
ever dispatched.

**React Aria is interaction semantics only.** It is imported only by
`primitives/aria.tsx` and `presentation/settings/SettingsRoot.tsx`. It owns
keyboard navigation, roving focus, dialog focus containment and restoration,
tab/grid/select/menu/disclosure/switch semantics. The page tabs are a vertical
rail (ArrowUp/ArrowDown); on a narrow panel the rail is hidden and the section
menu selects pages instead (see *Presentation (#393)*). No Spectrum stylesheet is
loaded; every visual token is the existing `--dsw-*` family
(`SettingsWorkflow.module.css`). Removal confirmations are `ModalOverlay`+`Dialog`
layers over the one Settings root — `role="alertdialog"` for a real deletion,
`role="dialog"` for restoring inheritance — with initial focus on Cancel; the
global WebUI modals are unchanged.

**Removal semantics.** User Settings removes an authored definition
(`data-removal="authored-removal"`, "Remove … from User configuration?").
Workspace Settings removes only the override (`data-removal="override-removal"`,
"Use the global default for …?") whenever something is inherited again; a
Workspace resource definition that shadows no User definition is a real deletion
and says so ("Remove … from Workspace configuration?"). Both send `authored: null` at the exact
revision after confirmation; cancelling sends nothing.

**Diagnostics.** Source paths and revisions, native unit names, application
observations, process bindings and raw projections are on Advanced. The only
per-unit revision detail is the collapsed *Source revision & replacement*
disclosure that the CAS review flow needs.

**Reference products.** The Provider → Provider detail → Model detail hierarchy
follows the public Z.ai ZCode configuration docs
(<https://zcode.z.ai/cn/docs/configuration>), and the page-per-task grouping
with a separate diagnostics surface follows the Kimi Web reference docs
(`MoonshotAI/kimi-cli` `docs/en/reference/kimi-web.md` at commit
`934b704a5eff1726623dd80db62907fbc1f7dd72`). Both were inspected only; no source,
asset or text was copied, and neither product's configuration precedence,
account or billing concepts were adopted.

## Presentation (#393)

#393 changes presentation only. No actor, transition, port, native DTO or
protocol version changed; the machines above are the same owners of reads,
mutations, settlement, fencing, navigation, observation and adoption.

**One frame with explicit regions.** `SettingsPanel` is one grid: the page rail,
a fixed header and one scrolling page pane. The header holds the owner label,
the observation lifecycle and **Reload configuration**, so every page shares
them and page changes never move or resize the frame (≈1000 px wide bounded by
the viewport, `min(820 px, viewport − 48 px)` high, r24). The header lives
outside React Aria `Tabs` on purpose: RAC renders a `Tabs` subtree a second time
into a detached collection document to discover its tabs, and the header's
section menu measures real DOM.

**Responsive layout is CSS, not state.** The panel is a size container
(`container: settings / inline-size`), and `@container settings (max-width:
680px)` decides the narrow layout from the panel's own width: the rail is hidden
and the header shows the section menu; content rules (field grids, rows, the
header context) follow the same query. There is no media-query store, resize
observer, breakpoint prop or machine state for it, and a narrow panel inside a
wide window is narrow. Only the modal shell's own viewport bound stays on a
media query. The deleted horizontal page strip, its `aria-orientation` styling
and its strip-scrolling tab label have no replacement.

**Section menu.** The narrow page selector is the shared rustX `Menu` (portaled,
`autoFocus`), whose rows are the same six pages with the same glyphs; selecting a
row sends the same `SELECT` the rail sends, the current row is marked with a
check and `aria-current`, and the trigger names the current page. Its
open/closed flag is the one transient state `SettingsPanel` holds.

**Floating geometry is Floating UI's.** `Menu`'s portal placement, flip, shift,
available size and anchor tracking are `@floating-ui/react-dom` (`offset`,
`flip`, `shift`, `size`, `autoUpdate`, fixed strategy); `side`/`align` map to one
placement and `getAnchorRect` is a virtual reference. There is no other
positioning path. Inside a React Aria modal a portaled list is marked
`data-react-aria-top-layer`, the attribute React Aria's modal focus containment,
`ariaHideOutside` and interact-outside detection honour, and Escape on an open
menu is stopped in the document capture phase so the modal never also sees it.
Tooltip and HoverCard keep their existing geometry.

**Modal mechanics stay React Aria's.** The nested-dialog and focus contracts
pass in real Chromium on React Aria (`settings-presentation.spec.ts`), so no
Radix Dialog or second modal runtime was added. Two focus defects of the nested
confirmation were found in Chromium and fixed inside `ConfirmAction`:

- its Cancel was focused by native `autoFocus` during commit, before the
  confirmation's focus scope registered as a child of the Settings scope, so
  the Settings scope took focus back; RAC `Button autoFocus` focuses from an
  effect after registration;
- closing it let focus fall to `<body>`, where React Aria's restoration and
  the Settings scope's containment raced over the next animation frame
  (occasionally landing on the Settings dialog or its first control — the
  long-standing intermittent failure of the keyboard reachability test). The
  layer now unmounts synchronously on close and hands focus straight back to
  its trigger, so focus never reaches `<body>` and nothing is left to race.

**Page vocabulary.** `SettingsContent.module.css` scopes one hierarchy to the
Settings section (`.page`): page title and description, group headings,
16 px-radius setting-row cards (≥ 56 px, 12 × 16 px padding) whose action row
closes the card and sticks to the pane bottom while the card holds a draft,
14 px-radius resource rows with the identity and its actions on one line and
secondary badges below, and a designed pending state for a target with no
projection. Toned badges, errors and alerts keep full-contrast text and carry
state as border/marker color, so the words carry the meaning. A resource detail
pins its back control to the top of the pane.

**Session configuration banner.** `SessionConfiguration` renders the same
predicates as before as compact lines — unavailable, preparing, ready, blocked,
failed — each with its state in text, a `StateDot`, the native reason or per-unit
detail and only its own actions; a container query moves actions below the text
on a narrow Session. Status lines are `role="status"`, read and adoption
failures `role="alert"`.

**Browser acceptance.** `test/e2e/settings-presentation.spec.ts` drives the
deterministic fixture through every primary page and the states above, asserting
geometry, frame stability, clipped and unclipped horizontal overflow, focus,
Escape ownership, touch, reduced motion and exact adoption requests, and runs
axe (`@axe-core/playwright`, WCAG 2.1 A/AA tags, serious/critical as failures,
no rule disabled) on each. `test/e2e/foundation.spec.ts` proves Floating UI
placement, flip, shift, bounded height and anchor tracking on the shared Menu.
