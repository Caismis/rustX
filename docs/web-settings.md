# Web Settings

Web owns forms, drafts and presentation. Rust owns parsing, semantic validation,
serialization, overlays, provenance, shadowing, CAS and runtime publication.
The [CFG3 reference](configuration.md) is the configuration contract.

Global Settings edits User definitions with no Session required. Workspace Settings
opens the exact registered Workspace through Product Host authorization without
changing Session focus or creating a runtime. The entry owns the target for the
whole lifetime of that Settings instance: the global entry is User authoring and
the Workspace object action opens `Workspace Settings — <workspace>` for that
exact authorized Workspace. There is no ordinary `Configuration owner` selector;
a Session focus change never retargets an open editor, and a revoked or
unregistered Workspace is fenced (its draft and error are retained) rather than
redirected to User or another Workspace. Resolved source previews are read-only and are
not the Session's adopted binding. The actual User pathname is displayed separately
from fixed `~/rustx/.agents`.

Settings is organized by product task into exactly six primary pages:

| Page | What it holds |
| --- | --- |
| **General** | Client-owned preferences: Appearance (theme). Global Settings opens here. |
| **Models** | The default model for new Sessions, then Provider list → Provider detail (endpoint, credential, its Models) → Model detail (the complete typed Model, reasoning profiles and protocol compatibility under progressive disclosure). |
| **Agent** | Root identity, description, instructions and project guidance. |
| **Tools & Permissions** | Approval mode, Native Tools, Tool-source selections, Skill visibility, the Agent and Workflow delegation allowlists, and per-Tool/MCP policies under **Advanced Tool policies**. |
| **Extensions** | One searchable inventory of MCP servers, Skills, named Agents, Workflows and Managed Python sources, filtered by kind, with a detail per resource; plus the native Todo, Goal and Agent Status extensions under **Native**. |
| **Advanced** | Context and runtime limits, environment identities, App Server process policy, native diagnostics (source paths, revisions, application observations, process bindings, raw projections) and **Rescan configuration files**. **Connection** is a sub-surface of Advanced, not a seventh page. |

The page list is a vertical rail on a wide screen (ArrowUp/ArrowDown) and a
horizontal strip on a narrow one (ArrowLeft/ArrowRight); the selected page is
kept in view in the strip.

Workspace Settings is a constrained surface of the same pages without General
(everything on General is client-owned) and without the User-only App Server
policy or Connection; it lands on Models. These are navigation capabilities of
the target, so no Workspace detail, page or action can reach them. There is no Overview page and no alias
for any earlier section route. Each primary page and each detail inside it is a
navigation fact of the one Settings navigation machine, so returning to a page
restores the detail the user left, and a new target starts with none.

Provider and Model edits address independent identities. Workspace
replacement edits complete objects. Native Tools use an explicit checkbox whitelist;
MCP, Python and Skill selections have all/exact/none controls. Skill guidance explains
prompt visibility and displays absolute collection roots. Plugins default off.
Named-Agent editing writes the complete resource profile, including its independent
Tools/Skills/Plugins, delegation lists, model inheritance, timeout and worktree settings.
Description and instructions remain optional; native projection and serialization
keep empty defaults omitted during unrelated profile edits. Root and named-Agent explicit Summary
Models each support independent model, reasoning profile, output limit and request
parameters. Changing Summary identity preserves its other authored fields; choosing
the Session/default option intentionally removes the explicit Summary settings.
The default model is the model *new* Sessions start from: saving it writes the
`root_model` source unit and sends no Session mutation, so an existing Session
keeps the model it was started or explicitly switched to. Each setting has
exactly one primary editor page.

**Save** submits a native typed semantic-unit mutation with the exact source
revision and transfers responsibility to native application. The browser keeps
four distinct stages: local draft plus exact base revision, native semantic-unit
mutation, committed acknowledgement, authoritative reread, and native per-unit
application observation. A write acknowledgement is never an application
observation, and a committed save followed by a failed reread is presented as
“saved plus current read/application status uncertain”, not as unsaved. Read
errors, write errors, CAS conflicts, application failures and unavailable/stale
observations are reported separately; a successful read clears only the read
error it answers. Connecting, loading, ready, stale and failed are distinct
states rather than one “loading” label. Change behavior (“Applies immediately” /
“Requires App Server restart”) is distinguished from the observed result
(Applied / Preparing / Failed / Restart pending), and any “Active” statement is
scoped to the native-confirmed unit rather than every Session. Conflicts preserve
the draft. Settings renders simultaneous applied, preparing, failed/retry,
process-restart state. The single **Adopt configuration** control lives below the
focused Session title, outside Settings. It sends the inspected candidate and
expected Session binding; native eligibility is advisory and the native admission
gate revalidates both. Failed preparation offers one owner-specific Settings
action per authored owner named by native `ConfigurationApplication.sources`.
That application's `scope` is the Session identity, never a source owner, so the
browser parses no scope string, infers nothing from the Session `cwd` and routes
nothing to a default owner. A Workspace owner the Product Host does not register
reports an explicit error and never falls back to User authoring; opening an
owning editor allocates no Workspace, Session or runtime, and a later Session
focus change never retargets it. Busy never cancels work or schedules
automatic adoption. Diagnostics offers native rescan. Uncertain writes/adoption
are repaired by rereading authority, never replayed. Reconnect rereads native
state.

Every Extensions row reports kind, owning scope and its relationship to this
target (authored, overriding, inherited or shadowed), definition validity,
native preparation or admission, and whether the root Agent may use it, as
separate facts. A valid definition is not a prepared one, a prepared one is not
one the root Agent may use, and a fact native did not publish is reported as
not observed — never as "no". Opening the page probes and connects nothing.

What a resource kind supports comes from a single native capability matrix
(`app/settings/capability.ts`). MCP definitions and named-Agent profiles have
structured editors. Skills, Workflows and Managed Python have no authoring
operation on this protocol, so their details show inventory, diagnostics and the
supported root selection with no Edit, Save or Delete for the definition itself.
Full Workflow program, Skill-package and Python-source editing are outside this
Settings scope.

A resource's definition and its root availability are two native mutations with
two outcomes. The resource detail edits the root selection through the same
semantic unit — one draft, one CAS base — as Tools & Permissions, and saves it
separately from the definition: saving one never submits the other, a rejected
or uncertain definition never produces an aggregate success, and nothing is
submitted automatically after an uncertain outcome.

Removal is confirmed in a modal dialog that states the actual native
consequence, and cancelling it writes nothing. In User Settings the action is
**Remove** (the authored definition is removed from User configuration). In
Workspace Settings it is **Use global default**, which removes only the
Workspace override so the inherited value applies again; it is never presented
as deleting the global definition.
MCP transport may be implicit: a URL shows the HTTP editor and a command shows
stdio. Ordinary edits preserve that authored omission and retained credentials.

## Browser evidence

These captures come from the real App Server/provider-emulator acceptance suite:

- [Effective values and native provenance](images/cfg3-effective-provenance.png)
- [Named-Agent source editor](images/cfg3-named-agent.png)
- [External-edit CAS conflict preserving the draft](images/cfg3-cas-conflict.png)
- [Failed application retaining the effective configuration](images/cfg3-application-failed.png)
- [Mobile named-Agent editor after publication](images/cfg3-settings-mobile.png)

## Harness integration

[Settings architecture](../web-console/SETTINGS-ARCHITECTURE.md) describes the one
Harness shell, the six product pages, Provider/Model drill-down, structured
exact-identity rows, the Extensions inventory and target-scoped unsaved drafts. A pure presentation
projection (`app/settings/projection.ts`) maps native facts into display state
without becoming authority: it never recomputes inheritance and never materializes
a native default into a draft.

The browser's asynchronous configuration semantics are owned by explicit XState
v5 actors under `app/settings/machines/`, not by component effects: the Settings
authority of one exact target (its authoritative reads, its one in-flight
mutation and its single level-triggered convergence obligation), one CAS editing
transaction per native semantic unit, Session observation and Session adoption,
and top-level Settings navigation. React subscribes and submits intent. Those
actors are addressed by `(endpoint, authority revision, subject)`, so a Settings
dialog closing is a detach — never a cancelled native commit and never a
discarded editing transaction — and replacing the App Server authority retires
the old lifetime at that replacement itself, observed by the configuration actor
system rather than discovered by a later actor lookup, without letting it
publish into or issue work through its replacement. Detach
retains editing transactions; attach revalidates authoritative observation: the
retained observation is stale presentation data and is never accepted as fresh
authority for a newly attached presentation. XState
orchestrates browser behaviour only; the native App Server remains the authority
for every value, conflict, application and adoption decision.

Read ordering is structural rather than compared: exactly one authoritative read
is in flight, and starting a newer one stops the older read actor, so a
superseded response has no completion path at all. The reread a Workspace write
owns is reserved when the write starts, and is silently superseded — in
success and in failure alike — by any read a newer native publication owes.
Only the publication of the target's own native source scope is such a
trigger: the Settings port projects the client's publications to that one
scope (`source:user`, or the Workspace directory its Product Host registration
resolves to), so a Session's or another Workspace's publication never retries,
refreshes or advances another target's reads.

Source mutations are admitted target-wide. While one semantic unit's mutation is
submitting, or until an authoritative read issued after its settlement (commit,
conflict, rejection or unknown outcome) has been adopted, every other unit stays
editable but cannot save or remove; nothing is queued, replayed or retried.
The pending write and its reread's authority to publish are separate facts. The
write always settles, even across a connection generation replacement; the
reservation survives a plain Settings close and reopen, but a newer read or a
replaced connection generation revokes it for good, so reopening Settings while
that write is still pending performs a fresh read instead of reviving it.

Replacing a connection generation retires only what that generation observed:
its projection, its read failure and its convergence report. A mutation outcome
— definitive commit, CAS conflict, native rejection or unknown outcome — is a
state of the Settings authority's mutation region, not of any generation, so it
reads the same whether the outcome or the replacement arrived first, and only a
new submission replaces it. Dirty intent, the pinned CAS base and a definitive
commit awaiting settlement live on the unit's transaction actor and survive the
replacement as well; an unknown outcome is reread, never replayed.

A definitive commit advances the transaction's CAS base to the committed
revision before any authoritative read has observed it. Until a read issued
after the commit completes, the transaction is *awaiting its commit
observation* and the difference from the held observation is not a source
change. Once such a read has completed, any revision other than the committed
one — including the exact pre-save revision restored by an external writer — is
an external divergence: the transaction reports it, requires review, and keeps
the next save fenced on the committed revision until the user explicitly adopts
the reviewed one. A read already in flight when the commit is acknowledged is
still adopted, but it never classifies that commit; only one that already
carries the committed revision observes it.

The Settings navigation machine is the one owner of which Settings surface is
displayed. Opening Settings, opening Connection, opening an owning Workspace,
selecting a page and focusing a detail inside it are all its events, so a
top-level decision taken while the dialog stays mounted is exactly what the
dialog shows. Page focus is presentation navigation only: no draft, CAS base or
mutation identity lives there, so leaving a detail never discards editing
intent.

React Aria Components provide the interaction semantics of the Settings dialog
(modal focus containment and restoration, page and filter tabs, resource grids,
selects, menus, disclosures, switches and confirmation dialogs) inside the one
Settings modal root; every visual decision stays in the existing rustX token
family. TanStack Form provides field mechanics for the complete typed Provider,
Model, MCP and named-Agent forms only: every change is reflected into the unit's
XState transaction, the form is rehydrated from that actor-owned draft, and Save
goes through the transaction. Neither library owns configuration state. TanStack
Form core's devtools event client, which would otherwise broadcast and queue
complete form state (including a typed literal credential) on `window`, is
replaced at build and test resolution by an inert module, and the production
build fails if that channel reappears.

Authored source state and effective resolution state are two independent
dimensions, not one state machine. "This Workspace authors no override" and "the
effective value is unset" are different claims: when a lower document does not
parse, native drops `resolved` and reports `prospective_diagnostic`, so the unit
is `authored=absent, effective=unavailable` and its effective value is shown as
unavailable rather than as `Unset`, an empty value or a native default. The valid
scope stays inspectable, repairable and authorable through exact CAS throughout.

An inherited Workspace unit displays the native effective value while authoring
nothing. Opening it creates no draft, and Save stays unavailable until an
explicit **Override** action or a real edit creates an override, so a no-op Save
can never convert "no Workspace override" into an explicit empty one.
**Use global default** means exactly "remove the semantic unit this Workspace
authors, through exact CAS": it sends `authored: null` at the reviewed revision,
never copies the User value, and is offered only while this Workspace really
authors the unit — an already-inherited unit offers Override instead, because
removing an absent unit has no meaning. Authored presence is the native
projection fact, never a truthiness test, so an explicitly authored `[]`, `{}`,
`false` or `""` is an override like any other and remains that exact value.
In User Settings the same action is **Remove**, since User authoring inherits
from nothing. **Discard draft** returns to the inherited presentation.

Identity discovery is a native fact too. Every Provider and Model identity in
the native effective projection is listed in Workspace Settings, with the
identity's native origin, even when this Workspace authors no override for it —
the identity never has to be retyped to be reached. Whole-file resource families
(MCP definitions, named Agent profiles) are enumerated from the native resource
inventory, which names the winning scope of each identity, so an inherited
definition is discoverable without the browser merging two catalogs. Opening an
inherited identity authors nothing: the displayed value is the native effective
one, and an override begins from a safe authoring seed. An inherited MCP
definition or named Agent is shown read-only; **Override … in this Workspace** is
the only way to begin a Workspace definition, which replaces the whole User one
when saved. Its seed never copies an inherited literal environment value or
header. **Use global default** removes that override and the User definition
applies again; a Workspace-only definition is removed with **Remove**. A Provider is the one
whole unit that inherits no editing state at all — an inherited credential stays
redacted, is never read back from the shadowed definition, and an override always
authors a new credential. User Settings is the lowest authored source and
inherits from nothing, so its catalogs list exactly what it authors.

No projection that leaves native authority carries a secret-bearing authored
value. A Provider credential is a redacted `CredentialSourceView`; an MCP
definition's literal `env`/`headers` are cleared natively and only their retained
identities are named; and a literal Tool environment value is projected as its
identity alone — `RuntimeLayer.environment` is a list of names on the wire, not a
map of values. The browser therefore knows that an environment identity exists,
which document authors it and what its provenance is, and authors an override by
entering a new value rather than reading the lower owner's value back. The
authoring document keeps the literal, which is what the running Tool environment
resolves from.

A submitted mutation may still carry an authored secret, because the user just
typed it. Before submission that value lives in exactly one place, the live
editing draft. The unit's transaction actor is handed only the mutation's
non-sensitive revision selector — the mutation family, plus a named resource's
identity — which is all settlement needs to match a commit against a later
authoritative projection. Once native acknowledges the commit, the confirmed
draft is dropped, so no secret-bearing authored payload survives a confirmed
commit, even when the post-write authoritative reread fails and the transaction
stays unsettled across an editor unmount or a page change.

Provenance is looked up by the exact native key a semantic unit owns
(`providers.<id>`, `models.<id>`, `agent.tools.sources.<id>`,
`native_tools.<id>`, `mcp_tool_policies.<id>`, `environment.<name>`,
`mcp_servers.<id>`, and whole-unit paths such as `agent.model`,
`agent.tools.builtin` or `context`). Sibling identities are independent: one
Provider resolving to User never depends on another Provider's name or length. A
container whose members genuinely disagree reports mixed origins instead of
electing one. Process settings are User-only and consume native hot/restart
classification. Theme is an intentionally browser-local preference. Resource
discovery never implies Root selection or native preparation. Source paths,
revisions, native unit names and raw projections are diagnostics: they are on
Advanced (and, per unit, behind a collapsed **Source revision & replacement**
disclosure), so the ordinary pages need no protocol vocabulary.

The current image references are under
[`settings-presentation.spec.ts-snapshots`](../web-console/test/e2e/settings-presentation.spec.ts-snapshots).
The earlier CFG3 captures above document the native contracts before the Harness
presentation migration. Real-server browser runs continue to capture Save, CAS,
publication failure and narrow Settings evidence in `web-console/test-results`.
