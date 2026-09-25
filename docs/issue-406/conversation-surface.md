# WEB-16: resident conversation ownership

PR #409 terminal-process follow-up: [current ownership contract](terminal-process-ownership.md).
The earlier terminal-marker representation described below is superseded by
native `TurnProcessView` in App Server v23 / Runtime Client v49. Follow-up
validation is recorded in the PR report.


Starting rustX: `ada60feccd6d3bd058a10f8e9b6418aa3f148a61` (`origin/main`).
Reference checkout: `/home/caismis/Documents/codes/deepseek-harness`, read-only.
Reviewed Harness: `477b4f420553e8a52c2fbccc464d7561b239c443`; local HEAD,
master and fetched origin/master agreed. No Harness product files changed.

## Ownership change

Previously App subscribed to the entire client snapshot, owned both a keyed
NewConversation tree and a separate active composer, and published generic
mutation notices. The first-submit actor's phases became product rows. Model
intent had no named durable product preference. Completed response controls
were message actions behind a Lineage menu. Ordinary status was shell-level.

Now the ordinary surface is one resident tree:

```text
App / conversation surface (bounded chrome subscription)
  Header (title, tabs, authority recovery; no cwd)
  ConversationLive (its own native Session subscription)
    ChatViewport / Trajectory
    native entry and Turn-tail nodes
    turn-local process / elapsed clock
  ComposerSeat (resident across first creation)
    ConversationComposer (explicit draft binding)
      WorkspaceControls (Host-authorized source + permission CAS)
      ComposerContextStack
        ConversationDocks (independent native subscription)
        AgentComposer (one draft / attachments / input trigger)
          AgentControls / ModelSelect
    ConversationTotals (native aggregates, independent subscription)
```

| State | Final owner / source / commit |
| --- | --- |
| Conversation surface | App resident section/header/body/seat; navigation changes explicit binding |
| Composer | ConversationComposer renders one unconditional AgentComposer |
| Session binding | App navigation + native Session/attachment identities; first-submit commit preserves draft binding |
| Draft and attachments | AgentComposer; explicit binding transition resets; asynchronous results are binding-fenced |
| First submit | firstSubmitMachine/Port; exact create acknowledgement commits Session identity, then model/upload/send; no replay |
| Input trigger | useInputTrigger reducer for typed and synthetic launcher hits, query/highlight/dismiss; launcher never edits draft |
| Workspace context | Product Host authorization; WorkspaceControls source observation and Settings CAS actor |
| Model selection | Native Session projection after creation; pre-creation intent must pass Workspace catalog admission |
| Last-used model | NewSessionModelPreference, separate authority-scoped browser product storage `rustx-new-session-model-v1` |
| Streaming | AppServerClient authoritative replaceable snapshots; selected subscriptions at live transcript/docks/statistics/Inspector |
| Turn process | Exact native Attempt clock and completed-process membership; presentation disclosure local |
| Elapsed clock | TurnProcess interval, no AppServer publication or App state |
| Completed Turn tail | turnPresentation nodes keyed by native Conversation/Attempt origin; native closing boundary and retry identity |
| Conversation statistics | Native presentation-event fold across the complete Conversation, independent of transcript page |
| Global blocking feedback | Transport incompatibility/disconnection, durable authority failure, deletion recovery, unresolved uncertainty |
| Action-local feedback | Deletion confirmation, Workspace mutation dialog, model control, settings workflow, attachment draft, command panel |

The product preference stores only model/reasoning intent. It is not navigation
state, configuration, catalog authority, a Session override, or a cross-device
account setting. Native acknowledged explicit selection updates it; creation
copies it into that Session exactly once. Without a preference, the authored
root default comes from the same native creation capture as the Workspace
catalog. Missing model/profile IDs remain visible and block submission; they
are never renamed to a catalog fallback. Browser storage denied by the user
makes this optional preference memory-only; native Session durability is unaffected.

Explicit Session navigation may reset the draft and remount Session-specific
transcript/dock views. First creation must not remount the composer. An
attachment replacement may remount its transcript cache view, not ordinary
composer chrome. Queue/steer/cancel remain exact native operations. No browser
queue, cancellation owner, event fold, or compatibility mode was introduced.

`useClientSelector` caches equality-selected values across publications.
`selectShell` supplies only connection/authority, catalog/summary/navigation,
Workspace settings and unresolved global authority. Its `ShellView` type has no
execution snapshot. Composer admission, interactions, model changes, transcript,
Trace and Tool bodies belong to local subscribers. No debounce drops stream state.

## Ten-area implementation and regression map

| #406 | Implementation / key files | Deterministic coverage / result |
| --- | --- | --- |
| A resident lifecycle | App, ConversationComposer, AgentComposer, first-submit | conversation-residency: held create/attach/send keeps exact input/card/seat and focus; IME does not submit |
| B one trigger | composer/input-trigger, AgentComposer | same reducer for + and slash; unrelated draft and selection unchanged on both surfaces |
| C internal phases | ConversationComposer actor projection | no creating/attaching rows while each native acknowledgement is held; local aria-busy and actionable failure |
| D streaming boundary | client/selectors, ConversationLive, LiveInspector | actual AppFrame and AgentComposer function spies remain unchanged over five exact snapshot updates; DOM identity/caret retained |
| E turn-local state | TurnProcess, AgentTranscript, session-product | fake-clock exact Attempt test advances five seconds without shell/composer invocation; no Working label |
| F model preference | model-preference, AgentControls, commands/native, native configuration projection | successful native selection seeds next hero; another Session retains model; unavailable preference blocks without create/write |
| G header | App | session-surface/presentation and native browser checks assert no Session location header |
| H Turn tail | turn-presentation, TurnTail, AgentTranscript | response-tail tests assert tail outside message article, exact origin and direct actions; real native branch/fork/retry browser path |
| I statistics | runtime_client/response, ConversationStats | native full/latest/older page equality test; response-tail test uses native totals with partial transcript |
| J feedback | SessionDeletion, WorkspaceNavigation, model/settings/upload owners | session-surface deletion closes with no success banner; client/deletion and first-submit tests retain uncertain effects and prohibit replay |

Existing composer-context, submission-policy, first-submit, commands, client,
session-surface and session-product suites continue to exercise queue/steer,
cancellation admission/settlement, exact upload receipts, stale generations,
uncertain writes, ordered native input restoration, IME and draft isolation.
Browser convergence additionally retains real DOM handles across first submit;
native command, upload, cancellation and response-loss scenarios exercise the
same production client rather than a second UI implementation.

## Removed paths

- Separate `NewConversation` component and navigation-epoch composer keys.
- `NewConversationModelControl`; both bindings use AgentControls/ModelSelect.
- `ResponseTail` message-action owner and its Lineage disclosure/menu.
- First-submit phase-copy rows and normal shell `Working…` / model-update rows.
- Raw cwd from the ordinary header.
- Old statistics footer hierarchy and confirmed-delete generic success banner.
- App Server v21 generated artifacts (mandatory v22, no compatibility shim).

## Convergence and deliberate native differences

Same interaction/presentation contract: resident composer seat, synthetic +
launcher, shared slash grammar, bounded local busy/error state, turn-local
process clock, completed Turn actions, native total statistics pills and local
details, no successful-delete banner. Process-row CSS follows the current
Harness typography, chevron/disabled/hover behavior and reduced-motion rule.

Native adaptations, not legacy concessions:

- rustX has no Session until authorized native creation. Harness's shell/editor
  residency is represented by an explicit draft binding and first-submit actor,
  not Cordis or a fabricated native Session.
- The native flat editor admits only its supported ordered input. Lexical chips,
  plugin claims and arbitrary reference serialization are not native contracts.
  Before creation, `/model` is available; commands requiring a native Session
  are unavailable rather than routed to invented IDs.
- Native Attempt starts map to product Turns; native Loop Turn starts map to
  Steps. Exact identities/phases remain in Inspector. Clock timestamps come
  from native events, not browser observation time.
- rustX distinguishes in-Session branch, independent Session fork and exact
  retry; direct tail actions retain those separate native operations.
- Catalog admission and authored defaults are native. The last-used choice is
  explicitly a browser product preference, not a rewrite of `rustx.toml`.
- rustX deletion is permanent, not Harness Session archive. Exact native preview
  revision, stop/quiescence, committed cleanup and uncertainty remain inspectable.
- Native uncertainty cannot be dismissed by a success toast or replayed; it
  remains durable recovery UI even after the initiating dialog is closed.
- Statistics describe the native Conversation execution epoch. Inherited
  lineage tails retain their original response usage/clock, while an unexecuted
  child Conversation has zero new Turns/Steps, independent of visible history.

## Protocol boundary

App Server v22 / Runtime Client v48 add required Turn/Step statistics and optional
measured timing/latest Turn clock. `SessionModelsView::Available.default_model`
is required. Generator, Web, TUI, transports and exact-version fixtures move
together. There is no Agent Loop semantic change, new execution store or database
migration. SQLite's presentation-event query already contains native Turn starts.

See [Web provenance](../../web-console/PROVENANCE.md#406-current-harness-conversation-contract)
for the exact studied and materially adapted source inventory. Validation and
browser evidence are recorded in the PR against the final branch revision.

## PR #409 review corrections

Terminal execution history belongs to the Event Journal. Committing an
`AttemptCancelled`, `AttemptFailed`, `AttemptTimedOut`, or
`AttemptLimitExceeded` atomically adds an `attempt_terminal` reference to the
existing transcript ordering spine. The reference contains no duplicated body:
the native reader resolves the journal envelope and projects Conversation,
Attempt and event identity, the exact outcome, and terminal timestamp. The
response fold supplies the matching native start timestamp. The transcript
cursor supplies stable ordering and bounded paging, including attempts without
Assistant output. Runtime reconstruction and resubscription use the same read
path. Successful Stop/Refusal response/process ownership is unchanged.

`AgentTranscript` renders historical Stopped/Failed only from these projected
items. Live Attempt state renders only an active process; absence or settlement
of that state cannot manufacture history. TUI consumes the same terminal item.
No execution history was added to React state or browser persistence.

`selectShell` returns a typed `ShellView` without execution snapshots, Attempt
identity, phase, stream, tools, queue, or model mutation state. `ConversationSeat`
owns composer admission/send/stop subscriptions, `ConversationLive` owns
transcript/trace, and `ConversationStatus` owns recovery. `TurnProcess` keeps its
clock local. Sidebar activity subscribes within individual rows; its browser,
`SidebarRoot`, `AppFrame`, and `ConversationHeader` stay catalog/chrome-owned.
The Session actions menu checks lineage eligibility locally and each command
rechecks the current native view at invocation. This preserves native admission
without a stale shell snapshot or extra memoization.

### Feedback audit

| Former App `run()` caller | Owner after correction |
| --- | --- |
| Transcript pagination | `history.error`, rendered inside transcript; rejected read is consumed there |
| Trace pagination | `trace.error`, rendered inside Trajectory; rejected read is consumed there |
| Cancel Turn | ConversationSeat action error; uncertain native outcome remains recovery-owned |
| Respond/cancel interaction | Interactions action error; native operation uncertainty stays visible |
| Export Session | Session header action error |
| Owning Settings lookup (formerly also subscribed by App) | Settings navigation actor, rendered beside the Session configuration action |
| Attach/classify selected Session | Global: failure invalidates the active surface |
| Release/close views | Global: failure leaves attachment authority unresolved |
| Retry Session refresh | Global recovery for an invalidated active surface |
| Deletion recovery | Global durable-authority/cleanup uncertainty |

CommandPanel, Workspace mutations, Session deletion, settings, uploads and model
selection retain their existing local owners. `runGlobal` is no longer passed
into conversation components. Normal terminal failures are not independently
reconstructed by `deriveSessionProductState`.

Deterministic evidence: the native terminal fold test reopens SQLite, compares
one-row pages and subsequent subscriptions, and adds later successful/running
Attempts. Web tests exercise idle → admitted → running → streaming → settled →
next Attempt for all four non-success outcomes, then reconnect. Spies execute
the real AppFrame, SidebarRoot, WorkspaceNavigation and ConversationHeader
functions. Local-read tests exercise rejected protocol responses and verify one
local alert, no App notice, plus preserved active-surface/global recovery.
