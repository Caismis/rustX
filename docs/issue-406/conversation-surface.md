# WEB-16: resident conversation ownership

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
`sameChrome` names the bounded facts allowed to invalidate App: connection and
authority, catalog/summary/navigation, admission/interaction/uncertainty, native
model and configuration revision. It excludes transcript, Trace, token and Tool
bodies. Live consumers independently subscribe; no debounce drops stream state.

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
