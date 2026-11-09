# Web Settings

Web owns forms, drafts and presentation. Rust owns parsing, semantic validation,
serialization, overlays, provenance, shadowing, CAS and runtime publication.
The [CFG3 reference](configuration.md) is the configuration contract.

The Session Model/Profile picker changes only that Session through
`session/setModel` and remains available during active execution when its
attachment, catalog and mutation state are valid. Its selected value describes
future independent Attempts. The running Attempt keeps its separately published
Model/Profile and effective invocation, including when the Model ID is unchanged.
Native confirmation establishes success; rejected or uncertain mutations do not
optimistically rewrite either authority. Recovery rereads native state without
replaying the mutation. Profile changes preserve independent Session overrides
and Summary Model policy; choosing another Model resets only primary selection
overrides. Saved context and capability changes commit natively at the Session's
safe boundary; no browser control confirms them.

Global Settings edits User definitions with no Session required. Workspace Settings
opens the exact registered Workspace through Product Host authorization without
changing Session focus or creating a runtime. The entry owns the target for the
whole lifetime of that Settings instance: the global entry is User authoring and
the Workspace object action opens `Workspace Settings — <workspace>` for that
exact authorized Workspace. There is no ordinary `Configuration owner` selector;
a Session focus change never retargets an open editor, and a revoked or
unregistered Workspace is fenced (its draft and error are retained) rather than
redirected to User or another Workspace.

Settings is organized by product task into seven sections:

| Page | What it holds |
| --- | --- |
| **General** | Browser-owned preferences: Appearance (theme) and Language (English / 中文). Explicit User Settings opens here; the primary Settings item resumes its previous section. |
| **Models** | Provider list → Provider detail (icon, Base URL, shared API format, API Key and its Models; credential source under Connection options) → one Model dialog (identity, limits, then capabilities and Model Profiles under Advanced). All Models and the default model for new Sessions follow the Provider panel. |
| **Agent** | Root identity, description, instructions and project guidance. |
| **Tools & Permissions** | Approval mode, Native Tools, Tool-source selections, Skill visibility, the Agent and Workflow delegation allowlists, and per-Tool/MCP policies under **Advanced Tool policies**. |
| **MCP Servers** | Native MCP source definitions, permissions and observation for the exact Settings owner. |
| **Extensions** | One searchable inventory of Skills, named Agents, Workflows and Managed Python sources, filtered by kind, with a detail per resource; plus the native Todo, Goal and Agent Status extensions under **Built-in**. |
| **Advanced** | Context limits, timeouts, concurrent subagent limits, environment variables and **Rescan configuration files**. **Auto Repair** appears for a repair receipt, a malformed document or an explicit inspection request. **Connection** is a sub-surface of Advanced, not a separate Settings section. |

Provider icons can be searched and selected from the locally bundled LobeHub catalog.
The icon is automatically saved with the Provider under the same scope rules,
and changes presentation only. The default icon clears that optional selection.

A Provider without Models is valid setup in progress. Its valid connection edits
are automatically saved after an idle interval or field completion; credential
text waits for blur. Pending generations belong to the existing unit actor,
survive detail unmount and wait for target-wide admission and an exact reviewed
revision. Failed submissions are not automatically retried. The Model dialog owns
the complete creation draft and validates required fields, duplicate identities
and token limits before submitting; it
closes only after its submission is exactly observed with no newer draft
generation or unfinished editing intent. Native settlement of an older
submission cannot close a newer editing flow. The shared bridge pairs completion
with the exact identity/token accepted for that editing flow; a failed MCP
creation renamed to an existing server cannot consume that server's earlier
completion. Every Model inherits its Provider's
API format. Existing credentials stay redacted, and changing credential mode
replaces the previous mode's fields.

Settings shows controls and actionable feedback. Internal revision hashes, raw
projections, process bindings, server process budgets and per-unit configuration
metadata are not product UI. File locations appear when needed to repair a
malformed document or manage a file-based resource.

Settings is an independent application page beside the permanent primary rail.
Its sections fill the available content area, with a fixed
header naming the owner (User or `Workspace Settings — <workspace>`), the state
of its authoritative observation and **Reload configuration**, and one scrolling
page pane. Changing page never resizes or moves the frame. Each page has its own
glyph from the existing icon family; the label, never the glyph or a color,
names it.

The panel chooses its navigation from its own usable width, not the window's:
a wide panel shows a vertical page rail (ArrowUp/ArrowDown, one tab stop); a
narrow one (390 × 844 phones included) replaces the rail with a section menu in
the header, which names the current page, opens from pointer, touch or keyboard,
marks the current page, and returns focus to its trigger. If the panel widens
while the section menu is open, the menu closes and the Settings page keeps
the keyboard. Escape closes a section menu or transient confirmation; the
Settings page has no backdrop, close control or Escape-close behavior.
Resource pages are list → detail at every width; a detail replaces its list and
keeps **← Models** / **← Extensions** pinned at the top of the pane. A unit card
with a draft keeps its **Save** / **Discard** row pinned at the bottom of the pane
while its fields scroll, and long identities, endpoints, paths and native
diagnostics wrap instead of widening the page.

Workspace Settings is a constrained surface of the same pages without General
(everything on General is client-owned) and without
Connection; it lands on Models. These are navigation capabilities of
the target, so no Workspace detail, page or action can reach them. There is no Overview page and no alias
for any earlier section route. Each primary page and each detail inside it is a
navigation fact of the one Settings navigation machine, so returning to a page
restores the detail the user left, and a new target starts with none.

Provider and Model edits address independent identities. Workspace
replacement edits complete objects. Native Tools use an explicit checkbox whitelist;
MCP, Python and Skill selections have all/exact/none controls. Skill guidance explains
availability. Plugins default off.

Freezing dynamic all-Skill visibility into an exact list requires complete,
successful native discovery, including every page. Loading or failed discovery
disables only that conversion; it never supplies an empty list. Existing exact
selections, explicit All/None and restoring Workspace inheritance remain
independent of discovery availability.

The Model capability editor contains one **Structured output** checkbox
(`json_schema_output`), defaulting to false. Its declaration means native API schema
enforcement verified for the selected model, endpoint, protocol and usable Profiles.
It follows the same whole-Model replacement and User/Workspace precedence as other
capabilities. Enabling it does not change ordinary Agent, Title or Summary requests.

Named-Agent editing writes the complete resource profile, including its independent
Tools/Skills/Plugins, delegation lists, model inheritance and timeout. There is no
worktree setting: every Agent runs in the shared Session workspace (#462).
Description and instructions remain optional; native projection and serialization
keep empty defaults omitted during unrelated profile edits. Root and named-Agent explicit Summary
Models each support independent model, profile, output limit and request
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
the draft. The Settings actor retains simultaneous application states; ordinary forms show
only actionable failures and their own save outcome. There is no adoption control: native commits a prepared
candidate automatically at the Session's safe boundary, and the compact status
line below the focused Session title only observes it. Failed preparation offers one owner-specific Settings
action per authored owner named by native `ConfigurationApplication.sources`.
That application's `scope` is the Session identity, never a source owner, so the
browser parses no scope string, infers nothing from the Session `cwd` and routes
nothing to a default owner. A Workspace owner the Product Host does not register
reports an explicit error and never falls back to User authoring; opening an
owning editor allocates no Workspace, Session or runtime, and a later Session
focus change never retargets it. Waiting for current work never cancels it.
Advanced offers configuration rescan. Uncertain writes and commits are repaired by rereading authority, never replayed. Reconnect rereads native
state.

Extensions lists names, descriptions, kind and agent access, grouped by scope.
Details retain scope relationships and actionable configuration errors without
healthy validity, preparation or admission badges. Unknown observations do not
become negative statuses. Opening the page probes and connects nothing.

What a resource kind supports comes from a single native capability matrix
(`app/settings/capability.ts`). MCP definitions and named-Agent profiles have
structured editors. Skills and Managed Python have no authoring operation on
this protocol, so their details show inventory, diagnostics and the supported
root selection with no Edit, Save or Delete for the definition itself. Workflow
files are authored in the **Workflows** destination of the primary rail (see
[Workflow authoring](workflow-authoring.md#file-authoring-app-server-and-web));
Settings keeps their inventory and root selection only. Skill-package and
Python-source editing are outside this Settings scope.

The Skills module exposes root-agent access separately from read-only skill
definitions. Its policy distinguishes dynamic `all` (including future discovery),
an exact list, explicit none (`[]`), and Workspace inheritance. Card toggles
author exact lists. Choosing the global policy in a Workspace removes its
authored override with `authored: null`; selecting all writes `all` explicitly.

A resource's definition and its root availability are two native mutations with
two outcomes. The resource detail edits the root selection through the same
semantic unit — one draft, one CAS base — as Tools & Permissions, and saves it
separately from the definition: saving one never submits the other, a rejected
or uncertain definition never produces an aggregate success, and nothing is
submitted automatically after an uncertain outcome.

Removal is confirmed in a modal dialog that states the actual native
consequence, and cancelling it writes nothing and returns focus to the button
that opened it. Confirming it moves focus to the setting's card, which reports
the outcome while its controls stay closed until native answers. In User Settings the action is
**Delete** for resources, or **Reset to default** for settings: a
destructive alert dialog with a trash glyph on its trigger and a destructive
confirm action. In Workspace Settings it is **Use global default**, which removes
only the Workspace override so the inherited value applies again; it is an
ordinary dialog with an ordinary primary action and is never presented as
deleting the global definition.
MCP transport may be implicit: a URL shows the HTTP editor and a command shows
stdio. Ordinary edits preserve that authored omission and retained credentials.

## Browser evidence

These captures come from the real App Server/provider-emulator acceptance suite:

- [Provider settings, Chinese dark desktop](images/settings-models.png)
- [LobeHub provider icon picker, dark desktop](images/settings-provider-icons.png)
- [Add Model dialog, Chinese dark desktop](images/settings-add-model.png)

## Harness integration

[Settings architecture](../web-console/SETTINGS-ARCHITECTURE.md) describes the one
Harness shell, the product sections, Provider/Model drill-down, structured
exact-identity rows, the Extensions inventory and target-scoped unsaved drafts. A pure presentation
projection (`app/settings/projection.ts`) maps native facts into display state
without becoming authority: it never recomputes inheritance and never materializes
a native default into a draft.

The browser's asynchronous configuration semantics are owned by explicit XState
v5 actors under `app/settings/machines/`, not by component effects: the Settings
authority of one exact target (its authoritative reads, its one in-flight
mutation and its single level-triggered convergence obligation), one CAS editing
transaction per native semantic unit, Session observation, and internal Settings navigation. React subscribes and submits intent. Those
actors are addressed by `(endpoint, authority revision, subject)`. The Settings
page remains mounted across primary navigation, so its configuration actors keep
settling. An actual editor unmount detaches observation without cancelling a
native commit or discarding editing transactions. Replacing the App Server authority retires
the old lifetime at that replacement itself, observed by the configuration actor
system rather than discovered by a later actor lookup, without letting it
publish into or issue work through its replacement. Detach
retains editing transactions; attach revalidates authoritative observation: the
retained observation is stale presentation data and is never accepted as fresh
authority for a newly attached presentation. XState
orchestrates browser behaviour only; the native App Server remains the authority
for every value, conflict, application and commit decision.

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
reservation survives leaving and returning to the retained Settings page. A
newer read or replaced connection generation revokes that read reservation for
good; later observation performs a fresh read rather than reviving an old one.

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

Resource editors consume the shared unit's exact settlement fact before
automatically closing. Completing a post-commit read alone cannot close an
editor whose transaction diverged. That editor keeps its source-change notice
and review action visible, including when another writer restored the original
source bytes. Reviewing adopts the observed CAS base without replaying the
write or claiming that the committed draft still exists.

A semantic unit whose definitive commit still awaits authoritative observation
is not a valid source for a new same-unit draft: until that observation, its
editor presents the pre-commit value while its CAS base already names the
committed revision, so the unit's transaction refuses every edit and its fields
are disabled. Editing resumes from the observed source once the observation
settles the commit or reveals a divergence. Other units stay editable, and
still cannot submit until the observation arrives.

Shell owns primary page selection. The internal Settings navigation machine owns
the exact target, permitted section and detail displayed within Settings. Explicit
User, Workspace and Connection entry points select their requested target;
the primary Settings item resumes the previous target and section. The retained
page renders those decisions without separate visibility or section state.
Page focus is presentation navigation only: no draft, CAS base or
mutation identity lives there, so leaving a detail never discards editing
intent.

The shared Base UI-backed dialog primitive owns modal containment, layering,
Escape, dismissal and the explicit close-focus lifecycle for transient Settings
confirmations. Settings workflows provide the intended focus destination:
dismissal returns to the usable trigger (otherwise the unit form); confirmation
settles on the stable unit form while native writes are in flight.
These are terminal-focus contracts, observed after the dialog closes; transient
body focus during library teardown is not a product invariant. The public
`finalFocus` callback focuses the exact target and disables automatic restoration.
Cancel receives initial focus. React Aria retains page/filter tabs, resource
grids, selects, menus, disclosures and switches. All styling stays in the
existing rustX token family. The narrow section menu is the shared rustX Menu, whose
geometry — placement, flip, shift, available size and following its anchor, for
the list and for every submenu — is Floating UI's, within the Menu's design
dimensions. Neither library holds a page, target or draft: they own
browser mechanics only. TanStack Form provides field mechanics for the complete typed Provider,
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
applies again; a Workspace-only definition is removed with **Remove**. A named
Agent file that exists but does not parse is still that scope's definition — a
Workspace one still shadows the User one — so it is reported as invalid, never as
new or inherited, and is replaced or removed on its own revision. A Provider is the one
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
Each is a real rendered state of the deterministic Settings fixture
(`web-console/test/fixtures/settings.tsx`; its variants are named in the query
string: `scenario=conflict|loading|read-error`, `session=preparing|ready|blocked|failed`),
asserted together with the interaction, geometry, overflow and axe checks in
`web-console/test/e2e/settings-presentation.spec.ts`:

| Reference | State | Viewport | Theme |
| --- | --- | --- | --- |
| `settings-general-light` | User General | 1440 × 1000 | light |
| `settings-models-light` | Provider list with a long identity | 1440 × 1000 | light |
| `settings-provider-detail-light` | Provider detail | 1440 × 1000 | light |
| `settings-agent-light` | Agent | 1440 × 1000 | light |
| `settings-tools-light` | Tools & Permissions | 1440 × 1000 | light |
| `settings-extensions-light` | Extensions inventory with a native diagnostic | 1440 × 1000 | light |
| `settings-workspace-inherited-light` | Workspace, inherited unit | 1440 × 1000 | light |
| `settings-workspace-override-light` | Workspace, overridden unit with a draft | 1440 × 1000 | light |
| `settings-delete-confirm-light` | Destructive removal confirmation | 1440 × 1000 | light |
| `settings-conflict-light` | CAS conflict with the draft preserved | 1440 × 1000 | light |
| `settings-loading-light` | Source read outstanding | 1440 × 1000 | light |
| `settings-read-error-light` | Source read failed | 1440 × 1000 | light |
| `settings-advanced-dark` | Advanced diagnostics | 1440 × 1000 | dark |
| `settings-mobile-menu-dark` | Section menu open | 390 × 844 | dark |
| `settings-mobile-dark` | Models with long identities | 390 × 844 | dark |
| `settings-provider-detail-mobile-dark` | Provider detail, long identity | 390 × 844 | dark |
| `settings-agent-narrow-dark` | Workspace named-Agent detail | 390 × 844 | dark |
| `session-banner-preparing-light` | Session banner, preparing | 1440 × 1000 | light |
| `session-banner-waiting-light` | Session banner, applies after current work | 1440 × 1000 | light |
| `session-banner-failed-light` | Session banner, failed with owner actions | 1440 × 1000 | light |
| `session-banner-failed-mobile-dark` | Session banner, failed | 390 × 844 | dark |

Updating them is an explicit, reviewed action: `pnpm --dir web-console test:e2e:update`
regenerates every reference from a stable capture in the pinned browser
container; review each changed image before committing it. The earlier CFG3
captures above document the native contracts before the Harness presentation
migration. Real-server browser runs continue to capture Save, CAS, publication
failure and narrow Settings evidence in `web-console/test-results`.

## Distinct facts

Settings presents seven facts that are never collapsed into one another:

| Fact | Meaning |
| --- | --- |
| Effective | what native resolution produced for this target, or that it produced nothing |
| Authored | what this exact scope's source holds, independent of the effective value |
| Provenance | which native document supplied the effective value |
| Source commit | a definitive native acknowledgement of one semantic-unit mutation |
| Authoritative reread | the read after a commit that observes (or fails to observe) it |
| Application / preparation | the native per-unit consumer outcome (Applied, Preparing, Failed, Restart pending) |
| Session commit | native's automatic move of a Session onto a prepared candidate at its safe boundary, recorded as one `configuration_applied` fact |

A confirmed save is not an application, a successful read does not clear an
unrelated application failure, a valid resource is not a prepared one, and a
fact native did not report is shown as not observed rather than as a negative.

## Automatic repair notification

A native automatic repair of a `rustx.toml` source (#464) shows one brief,
nonblocking notification — for example **Configuration automatically repaired
· 3 changes in User configuration** — with **View details** and **Dismiss**.
Simultaneous User and Workspace repairs share one sentence; an unresolved or
uncertain repair is an alert naming its source. View details opens the exact
owner at Settings → Advanced → Auto Repair and acknowledges that source's receipt
only once those details are on screen, so the other source of a combined notice
stays presented, and an owner lookup that fails, leaving Settings or any other
navigation acknowledges nothing; Dismiss acknowledges the whole notice. Nothing
opens by itself and a healthy source shows nothing. Each receipt is acknowledged
once per browser for the App Server authority that produced it, so reconnects,
rereads and reloads never repeat it, while a second repair occurrence of the same
authority — even of identical changes — has its own id and is presented, and the
same receipt id from another App Server, or a new process at the same endpoint,
is presented on its own. Other
tabs' acknowledgements are merged before each write and picked up live; two
tabs acknowledging at the same instant can, at worst, show one notice once more. The notification is
presentation of a native fact: it never edits, submits or rebases a Settings
draft. See `SETTINGS-ARCHITECTURE.md` → Automatic repair.

## Session configuration banner

Below the Session title, one compact line per native fact, shown only while it
needs attention: **Preparing**, a prepared change that **applies after current
work**, **failed** preparation that kept the previous configuration (the native
diagnostic, one **Open … Settings** action per native owner and a native
**Retry**), an **uncertain** commit, or settings waiting for a **restart**. Lines
wrap on a narrow Session instead of truncating. Success is silent: the line
disappears once everything is applied, and unavailable is never "up to date".
Nothing here confirms, schedules or cancels work.

Committed changes appear as facts, never as status. Following the Harness
hidden-rows contract, Chat hides system-prompt and ordinary Context rows and
shows only committed Tool additions and removals, in the Harness `DisclosureRow`
style (`ConfigurationChangeRow`): one compact row for a single Tool, counts when
collapsed and grouped names when expanded for several. Trace shows every
committed change as a `Configuration` record with its domains and bounded names.

## Browser-owned language

General owns Language alongside Appearance; Workspace Settings has neither.
The page-level controller in `web-console/src/locale/controller.ts` is the sole
locale owner. React consumers subscribe to its immutable, revisioned snapshot
with `useSyncExternalStore`. Switching between `en` and `zh` updates the resident
UI immediately and sets `<html lang>` to `en` or `zh-CN`, respectively.

The versioned browser key `rustx-locale-v1` accepts only `en` or `zh`. A valid
stored preference wins; otherwise the first available browser language selects
Chinese for `zh` or `zh-*`, and English for everything else. Unavailable storage
and browser APIs fail safely to English. Explicit choices persist in browser
storage only. No native configuration, Session, Workspace or runtime mutation
is involved. Date/time/number formatting follows the selected rustX language.

Feature-owned, statically paired English/Chinese dictionaries live in
`web-console/src/locale/dictionaries`. A single typed translation path supports
small named placeholders. Deferred presentation messages retain their typed key
until rendering so existing notices also switch language. Native errors, raw
Inspector values, protocol/Tool/model/resource identities, paths, and user-,
model- or Tool-authored content remain opaque. Navigation and native operations
continue to use stable semantic IDs independently of translated labels. Command
and Trajectory search accept both built-in vocabularies regardless of the active
locale, so switching language cannot change search membership. Session
rename editors start from the native authored name, never the localized fallback
shown for an unnamed Session.

Run `pnpm --dir web-console check:i18n` to check authored JSX text and common
copy/accessibility props using the TypeScript AST. Intentional raw literals need
a narrow adjacent `i18n-raw` explanation; translation dictionaries own product
copy. The compiler checks exact built-in dictionary key sets. Browser E2E starts
with an explicit English preference; dedicated Chinese tests cover live switching,
persistence, composer commands and approval behavior.

### Observation ordering within one connection

Each Session actor orders native configuration publications independently of
application versions. A read captures the publication order when it starts; its
success (including absence) or failure may commit only if that order remains
current. A newer publication supersedes all earlier read completions, including
null and error. Leaving the connection generation cancels its read authority.
Known absence is a successful observation, not failed/unavailable authority.

A relevant failed reread remains visibly failed. An uncertain commit is never
replayed. Streaming deltas and configuration publications issue zero additional
configuration reads.
