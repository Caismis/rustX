# `rustx-tui`

The rustX reference terminal client: a Pi-TUI presentation layer over Runtime
Client Protocol v3.

## The one architectural rule

**Pi TUI is only the terminal input/output projection of rustX.**

It is not a runtime, not a session owner, not an agent framework, not a model
framework, not a tool runtime, not a capability authority, and not a second
source of conversation truth.

```text
user keyboard                          rustX Runtime
     |                                       |
     v                                       v
Pi Editor / controls              Runtime Client snapshot/events
     |                                       |
     v                                       v
rustX intent/command layer        rustX presentation projection
     |                                       |
     v                                       v
Runtime Client request            rustX presentation components
     |                                       |
     v                                       v
rustX Runtime                         Pi rendering primitives
                                             |
                                             v
                                          terminal
```

Pi provides terminal mechanics. rustX provides semantics. The dependency
direction is never reversed:

```text
rustX Runtime semantics
        -> Runtime Client Protocol
        -> rustX TypeScript projection
        -> rustX TUI presentation
        -> @earendil-works/pi-tui primitives
```

### The test for the layering

> If `@earendil-works/pi-tui` were replaced tomorrow with another terminal
> rendering library, would rustX Runtime Client semantics, protocol handling,
> presentation reduction, model state, tool state, background state, and
> command semantics remain valid?

Yes. Pi-TUI dependencies are confined to TUI presentation and input
components. Runtime Client protocol handling, projection semantics, Session
semantics, and execution semantics do not depend on Pi. Everything below
`src/ui/` is plain TypeScript over protocol values.

No suite needs a terminal. Framing, RPC, presentation projection, session
lifecycle, the model invariant, tool correlation, the process owner, and the
real `rustx` integration never reach `@earendil-works/pi-tui` at all, directly
or transitively; the presentation suites reach it only to render strings.

## Requirements

- **Node**: the nvm LTS line (`.nvmrc` says `lts/*`). The package declares
  `"engines": { "node": ">=22.19.0" }`, which is what `@earendil-works/pi-tui`
  requires.
- **Package manager**: pnpm, via Corepack. The pinned version is recorded in
  `packageManager`.

```sh
nvm install --lts
nvm use --lts
corepack enable
pnpm --dir tui install --frozen-lockfile
```

`npm install`, `npm ci`, `yarn`, and `bun` are not the project workflow; the
dependency graph and lockfile are owned by pnpm.

## Running

The TUI owns the lifecycle of the `rustx` child process and nothing else. Build
the binary first, then point the client at it and at the runtime's own
configuration paths:

```sh
cargo build --bin rustx

pnpm --dir tui start \
  --binary "$PWD/target/debug/rustx" \
  --models "$PWD/examples/local-runtime/models.jsonc" \
  --config "$PWD/examples/local-runtime/rustx.jsonc" \
  --workspace "$PWD/examples/local-runtime/workspace" \
  --runtime-root "$PWD/examples/local-runtime/.rustx"
```

The four startup paths are passed straight through. The `--config` path is
the current runtime/project configuration; after startup the native
SessionCatalog/SessionGraph under `--runtime-root` owns durable user sessions
and lineages. **The client never opens,
parses, or interprets any of them** — `models.jsonc` is a runtime-owned model
authority, and reading it here would create a second one. Provider credentials
are resolved by the Rust process from the environment it inherits. For the
complete copyable configuration and Python-tool example, see
[`examples/local-runtime/README.md`](../examples/local-runtime/README.md).

Starting the client is not resuming a Session. Every launch begins on an
empty Session, and the Sessions of earlier launches stay reachable through
`/resume`. `--continue` starts on the Session the last launch left active
instead. Rust owns that selection; the client only forwards the flag.

`--name <text>` names the Session the launch bound, whichever one that is —
the startup form of `/name`. It is metadata, not a Session request: it
combines with `--continue`, `--session`, and neither, and a replacement spawn
drops it so a later Session cannot inherit the label.

The TUI may also forward the runtime's bounded startup controls:
`--skill <path>` (repeatable), `--no-skills`, `--no-builtin-tools`,
`--no-tools`, `--tools <a,b,c>`, and `--exclude-tools <a,b,c>`. It preserves
their supplied values and order; Rust owns discovery, validation, activation,
and all semantic errors. Normal rustX composition keeps mandatory native Read
active even when these controls disable optional tools.

## Startup sequence

```text
spawn rustx
        -> initialize(v3)
  -> authoritative snapshot + cursor
  -> install the presentation projection
  -> subscribe_events(after cursor)
  -> interactive
```

Session replacement is also native-owned. A successful `/new`, `/resume`,
`/clone`, `/fork`, or `/tree` result may require replacing the child process;
the TUI closes the old attachment and the restarted Rust process re-reads the
authoritative catalog. A replacement spawn always passes `--continue`, because
it completes a transition Rust has already published — that is the difference
between replacing the process and launching the client, which starts on an
empty Session. A committed-but-durability-uncertain fork/tree result
also carries the selected user content as transient editor data. The TUI
restores it only after `session_get` confirms the restarted Session/node, and
it is not canonical until submitted. Tree node and history pages have
independent bounded continuations; an exhausted stream is never restarted
from an earlier offset while the other stream continues.

Before the first real transcript turn, the screen includes a compact welcome
block with the published effective model, protocol/provider display label,
context window, reasoning state, active Session name/node when the native
Session read is available, and the basic keyboard hints. Once a real turn or
durably accepted transcript content exists, that block is reclaimed and the
compact footer carries the durable Session metadata and live status instead.

## Owners

| Module | Owns | Does not own |
| --- | --- | --- |
| `runtime/child-process.ts` | spawn, stdio, bounded stderr tail, stdin close, wait, fallback termination | anything semantic; it never reads stdout |
| `protocol/jsonl.ts` | LF framing, CRLF, the 8 MiB bound in encoded bytes | protocol meaning |
| `runtime/connection.ts` | request ids, the pending RPC map, correlation, event delivery, ordered writes, terminal settlement | conversation state |
| `runtime/attachment.ts` | attach, snapshot install, subscribe, resync repair, shutdown | agent/session semantics |
| `presentation/projection.ts` | the ephemeral render cache and bounded transcript page | canonical history, authority of any kind |
| `presentation/tools.ts` | the `ToolCallId` correlation used for display | tool lifecycle, which it only reads |
| `presentation/todos.ts` | reading the runtime's task-list projection and folding newly committed `todo` results into it | task identity, status, and dependencies, which the runtime owns |
| `sanitize.ts` | the one rule for what externally-derived text may be handed to a terminal | what any of that text *means* |
| `commands/` | slash-command parsing, dispatch to canonical operations | parallel runtime semantics |
| `ui/components/` | the semantic presentation grammar | every fact it displays |
| `ui/preferences.ts` | reasoning visibility and expanded cards | anything the runtime owns |
| `ui/` | Pi components and rendering | every fact it displays |

## Presentation surfaces

The dispatcher classifies client information by presentation intent before it
reaches Pi. The app owns the rendering mechanics; none of these client-side
surfaces are Runtime Client facts or canonical conversation history.

| Surface | Semantics and lifetime |
| --- | --- |
| **Inspection** | One reusable focused viewport for substantial read-only Markdown. It is bounded and scrollable with Up/Down, PageUp/PageDown, Home/End, and Escape. |
| **Picker** | Existing focused selectors and approval interactions remain overlays with their existing selection and focus semantics. |
| **Transient** | One current item, owned by the app. New feedback replaces old feedback; any input acknowledges it, and attachment/session replacement clears it. Producers keep the payload compact enough for the three-line bound; a defensive overflow is marked explicitly, and no wall-clock timer is used. |
| **Local scrollback** | Deliberately not implemented. These client events have no honest interleaving point with runtime conversation history, so they use the finite transient surface instead of a second local event store. |
| **Task panel** | The task list the runtime published, drawn between the conversation and the editor because it answers a question the reader has while typing the next message. It is derived from the runtime's own snapshot projection and the committed `todo` results observed since, holds no state of its own, is bounded so a long plan cannot push the conversation off screen, and disappears entirely when the list is empty. Task text is sanitized before it is drawn, so one task is always one physical row and no model-written escape sequence reaches the terminal. |
| **Preference** | Reasoning visibility and expansion choices stay in client display preferences and never become runtime messages. |
| **Control** | Canonical commands still go through the Runtime Client. Their short acknowledgement is transient; runtime status and settlement remain authoritative runtime projection. |
| **Quit** | Shutdown is a control intent. Lifecycle failures are committed in a final Pi frame before the TUI stops, and are never turned into fake transcript messages. |

The command-to-surface classification is:

| Command | Final surface |
| --- | --- |
| `/help`, `/session`, `/tools`, `/skills`, `/todos`, `/status`, `/debug` | inspection |
| `/model show` | inspection |
| `/model` and `/model list` | picker; the selection result is transient |
| `/resume` | picker; a direct session id is a control operation with transient result |
| `/fork`, `/tree` | picker; the selected session operation has transient/replacement feedback |
| `/new`, `/clone` | control with transient/replacement feedback |
| `/name` | inspection of the active Session's name, as transient feedback |
| `/name <text>`, `/model <provider/model>` | control with transient result |
| `/cancel`, `/compact`, `/approve`, `/approval` | control with transient acceptance/validation result |
| `/reasoning`, `/expand` | preference |
| `/quit` | quit |
| invalid, unknown, or empty-result command feedback | transient |

Inspection and transient state live outside `PresentationState`: an
authoritative snapshot or event-stream resync reconstructs runtime-derived
projection state and does not carry arbitrary client feedback. Every such
replacement closes all focused overlays — inspection and every picker —
because their displayed facts or eventual selection action may have been
derived from the superseded attachment. It also clears the transient item
before the destination projection is shown.

The app owns a presentation epoch tied to the current attachment. Async
command, search, pagination, and selector continuations capture that lease
and may update local presentation only while the epoch and attachment still
match. Binding a replacement attachment, accepting a Session restart, or
installing an authoritative snapshot invalidates the old lease. The canonical
transcript remains reconstructed only from runtime-published durable message
and audit facts; client output never becomes a `MessageBlock`, a model request
payload, or a Runtime Client protocol event. User input is rendered as
transcript content only after durable acceptance. The snapshot contains only
the newest bounded transcript page; PageUp requests older pages with
`transcript_page_get` and merges them by durable transcript cursor. Entry
identity only detects the same durable fact; it never supplies ordering. The
transcript cursor is separate from the live event cursor, so loading older
history cannot disturb event resynchronization.

The wire validator and presentation reducer share one visibility contract:
only a hidden Context-kind User message may omit `transcript_cursor`. Visible
User, Assistant, Tool, and inbound facts require the durable cursor, while a
hidden Context carrying one is contradictory protocol state. The connection
closes on malformed wire input and the reducer fails closed when called
directly; neither path fabricates a cursor or changes accepted presentation
state.

Command routing has a separate ownership boundary: rebinding the dispatcher
changes admission only for future invocations. Each admitted command captures
its `RuntimeClientAttachment` before its first await and passes that exact
attachment through every later phase, so a catalog response from attachment A
cannot cause a follow-up mutation on attachment B.

## Commands

The TUI's current slash-command surface is grouped by purpose:

### Session lifecycle

A launch already starts on an empty Session; the commands below move between
Sessions afterwards.

- `/new` — create a new independent local Session.
- `/resume [session-id]` — search persisted Sessions, or activate the given
  Session directly.
- `/session` — show active Session metadata: name, id, node, conversation, and
  node count.
- `/name [text]` — show the active Session's name, or give it one. Sessions
  are unnamed by default; `/resume` lists an unnamed Session by its first
  message, and a name replaces that line without touching history, identity,
  or lineage. Nothing is ever resolved by a name.
- `/clone` — clone the committed conversation head into a new Session.
- `/fork` — choose a historical user message and open an editable fork in a
  new Session.
- `/tree` — inspect the active Session graph, select a node, or branch from a
  historical user message within the Session.

These operations use the runtime-owned Session catalog and graph through the
canonical Runtime Client operations. They may replace the attached `rustx`
process; the TUI reattaches and reprojects the authoritative result. The TUI
does not implement a parallel Session system.

### Model and capability inspection

- `/model [show|provider/model]` — open the searchable model selector, show the
  current model view, or select a model directly.
- `/tools` — show the runtime-published Active Tools and the Available but
  inactive Tools separately.
- `/skills` — show the active Skill catalog.
- `/todos` — print the complete task list the agent is tracking, grouped by
  status. The panel above the editor shows the same list, bounded.
- `/status` — show the runtime-composed Agent Status and diagnostics.

### Control and presentation

- `/cancel [execution-id]` — request cancellation of the current attempt, or
  a background execution by id.
- `/compact` — ask the runtime to compact the canonical context while idle;
  progress and completion remain authoritative Runtime Client facts.
- `/approve <interaction-id> <allow|deny> [reason]` — answer one runtime-owned
  Approval interaction.
- `/approval <policy|full_access>` — request the runtime ApprovalMode.
- `/debug` — show bounded presentation and protocol diagnostics.
- `/reasoning [on|off]` — change the display preference for model reasoning;
  it does not change runtime model configuration.
- `/expand [latest|all|none|<tool-call-id>|background <execution-id>|interaction <interaction-id>]` —
  expand or collapse display detail without re-executing or re-fetching.

### Lifecycle and help

- `/help` — list the available commands.
- `/quit` — shut down the runtime and exit cleanly.

Each either renders projection state, changes a client display preference, or
invokes exactly one canonical Runtime Client operation. `/model` opens the
searchable selector over `model_catalog_get` and applies a choice through
`model_set`, while `/model show` renders the projection's own model view;
`/tools`, `/skills`, and `/todos` read the projection the client already
holds; `/status` prints the
runtime's own Agent Status rendering; `/compact` invokes one
`compact_context` operation and waits for its durable terminal result;
`/debug` shows bounded diagnostics and
never a credential; `/approve` sends a finite typed response to one
runtime-owned Approval interaction, the questionnaire overlay sends one typed
whole-questionnaire response, and `/approval` requests a runtime control-plane
mode change. The TUI never
edits displayed Tool arguments, suppresses pending prompts, auto-answers them,
or keeps a local outcome.

`/reasoning [on|off]` and `/expand [latest|all|none|<tool-call-id>|background
<exec-id>|interaction <interaction-id>]` are the two commands that touch
nothing but the screen. They send no request, and they are also bound to keys:

| Key | Effect |
| --- | --- |
| `ctrl+l` | open the same model selector as `/model` |
| `escape` | the focused overlay closes first; otherwise request runtime cancellation while an attempt or interaction is unsettled |
| `ctrl+c` | request runtime cancellation while an attempt or interaction is active, or quit when idle |
| `ctrl+o` | expand or collapse the most recent tool card |
| `ctrl+t` | show or hide model reasoning |

Escape precedence is an app-level ownership rule: a focused inspection or
picker receives Escape first and closes, restoring editor focus. With no such
overlay, an unsettled attempt or pending interaction makes Escape a runtime
cancellation request; it never becomes an ordinary editor submission. The
same rule applies to Ctrl+C while an interaction is focused. Authoritative
resync uses the same rule in reverse by closing every overlay before any new
input is interpreted.

There is deliberately **no** `!bash`, no `@file` attachment, no client-side
file read, and no client-side Skill execution. Shell, file, and Skill behaviour
must travel through the real rustX tool and capability path, and rustX has not
yet defined a client-facing attachment contract.

## Native HITL

Pending Approval and Questionnaire interactions arrive as runtime-owned
`interaction_pending` events and in
the authoritative `snapshot.pending_interactions` view. The presentation
reducer sorts and replaces those facts like every other projection value;
reconnect/resync discards local assumptions and rebuilds the list from the
snapshot. The renderer shows the immutable tool identity, mode, reason, and
validated arguments for Approval, and the bounded questionnaire facts, option
descriptions, previews, review tab, and client-owned custom row. `/approve`
sends only an Approval response; the questionnaire overlay sends one typed
whole-questionnaire response. Neither invokes a Tool, mutates
arguments, infers an outcome from detach/EOF, or callbacks into the Agent
Loop.

When questionnaires are pending, the TUI focuses the questionnaire with the
smallest `InteractionId`. This selection is derived from the authoritative
projection, so it is stable across reconnect and resync. Plain editor input
always remains an ordinary inbound message. The focused questionnaire overlay
alone collects authored option selections or bounded custom text and sends one
typed whole-questionnaire submission; Approval remains explicitly
command-driven through `/approve`, and `/cancel` addresses attempt
cancellation. The TUI never settles, suppresses, or auto-answers an
interaction locally.

The focused questionnaire is a real Pi-TUI overlay. It reconstructs from
`snapshot.pending_interactions` after attachment/resync, shows one tab per
question plus a review/submit tab, renders option descriptions and Markdown
previews, and uses a side-by-side preview when the terminal is wide enough or
stacks it on narrow terminals. `Tab`/`Shift+Tab` changes tabs, arrows move
rows, `Enter` chooses or submits, and `Space` toggles a multi-select option.
Every question includes the client-owned `Type something.` row with a 4096
Unicode-scalar draft bound. Partial submission is valid; unanswered questions
remain visible in review. `Esc` explicitly declines only the focused
questionnaire and `Ctrl+C` keeps its existing meaning of cancelling the owning
attempt. A local draft is never converted into a response by detach, EOF, or
attachment replacement; a settled interaction closes the overlay and restores
editor focus.

Custom-answer editing delegates raw input to Pi-TUI 0.82.1's native `Input`,
so bracketed paste, multi-character batches, Kitty printable input,
grapheme-aware cursor movement, deletion, and the 4096-scalar bound share one
input path. The questionnaire component owns an explicit height-aware
viewport: every visible line is display-width bounded, the focused row and
Review/Submit remain reachable, and long Markdown previews can be inspected
with PageUp/PageDown without allowing preview content to displace the option
list.

The footer displays authoritative `approval policy` or `approval FULL ACCESS`
and shows a pending arrow when the runtime has accepted a busy-time mode
request. Full Access is not a local prompt suppression switch: it changes
only effective approval for eligible Tool calls. It cannot activate disabled
Tools, restore excluded Tools, bypass execution/concurrency restrictions, or
answer a pending interaction. `/tools` displays each Tool's independent
execution, concurrency, and approval policy; availability and activation are
shown separately.

## The model selector

`/model` and `Ctrl+L` open the same searchable overlay over the catalog
`model_catalog_get` published. The search field delegates cursor movement,
insertion, deletion, word editing, undo, kill/yank, and bracketed paste to
pi-tui's native `Input`; rustX still owns filtering/ranking over the catalog's
published facts, highlighted identity, and selection. Enter selects the
highlighted row and Esc closes the overlay. A visible overlay owns its input,
so the app-level Ctrl+L shortcut is ignored while it is open and Esc closes it
before cancellation is considered. The existing Ctrl+C interrupt-or-quit
policy remains unchanged.

The selector searches the model reference *and* useful metadata the catalog
publishes — protocol, modalities, capabilities, reasoning profiles, and
limits. It preserves configured, effective, and attempt-frozen identities and
shows the highlighted row's effective facts exactly as published. The client
does not read `models.jsonc`, infer provider behavior from a model prefix, or
invent a reasoning scale. Selecting a row calls the canonical dispatcher
`model_set` path; a `replacement_required` result is interpreted by the same
`#handleOutcome` flow as `/model` and Session commands.

The footer is intentionally compact: it keeps the effective/configured/
attempt-frozen model distinction, `session <name> · node <node>`, work state,
latest published token usage, queued/background/approval counts, optional
capability availability, connection state, and a short command hint as space
allows. The context indicator is specifically the latest runtime/provider-
published `input_tokens` divided by the published context window for that
attempt's model. It is not a client tokenization of transcript history and is
not a canonical compaction or occupancy calculation. Plumbing such as
attachment ids, cursors, and capability revisions remains in `/debug`.
During automatic or manual compaction, the footer displays the runtime-published
`Compacting context…` lifecycle state; it never infers that state from token
occupancy.

## The model invariant

`snapshot.model` is the session's *desired* configuration.
`snapshot.attempt.model` and the `attempt_started` event carry the *immutable*
model an already-admitted attempt froze. While an attempt runs on A and the
session is switched to B, the UI shows both truthfully and the running attempt
never visually mutates to B. The next admission shows B.

This is proven in `test/model-invariant.test.ts` through the pure reducer and
end to end over the transport, and again against the real binary in
`test/integration.test.ts`.

## The semantic presentation model

The transcript is a grammar of semantic components, not a log of protocol
records:

```text
user            ▌ the question, verbatim
assistant text  ordinary Markdown, no banner
reasoning       dimmed, or one `Thinking…` marker when hidden
refusal         explicitly a refusal, never an answer
tool_call       one correlated tool card
tool result     folded into that card when folding preserves canonical
                order, otherwise that card's continuation in place
publication audit  explicit non-canonical Assistant output; proposed tool
                   calls are unaccepted and unexecuted
interaction audit requested/settled historical evidence, never an active prompt
```

Canonical block order is preserved exactly: `reasoning, text, tool_call, text`
renders in that sequence, streaming and committed alike, and streaming text
renders identically to the committed text so nothing reflows on commit.

When the runtime publishes a terminal non-success attempt outcome, the
transcript renders a bounded inline band beside the interrupted conversation:
cancelled, timed out, limit exceeded, model/provider failure, or runtime
failure. A completed attempt adds no error band, and transport EOF, detach,
restart, or missing assistant text never gets reinterpreted as cancellation or
failure.

### One tool call is one visual entity

rustX publishes three different facts about one logical call:

```text
assistant tool_call block   committed conversation content
foreground execution        attempt-scoped execution lifecycle
tool result message         committed conversation content
```

Their semantic ownership stays separate. `presentation/tools.ts` joins them for
*display*, keyed by the runtime's own `ToolCallId` — never by tool name,
argument equality, list position, timing, or adjacency, so two concurrent calls
of the same tool with the same arguments stay two cards. The card renders at
the assistant block that asked for the call and evolves in place:

```text
◇ Bash · preparing        ->  ◐ Bash · running · 40/900  ->  ✓ Bash · ok · 2.8s · exit 0
```

### One entity, canonical order

rustX's canonical model permits a `tool_call` that is *not* the last block of
its assistant message: `AssistantMessageBlock` holds a plain block vector, and
`StructuralIndex::build` rejects only duplicate calls, duplicate results, and
orphan results. So `text A, tool_call X, text B` followed by a result for `X`
is a shape the TUI must render, not one it may assume away — and drawing that
result inside the earlier `X` card would move it before `text B`.

Nor does it require a result message to immediately follow the message that
requested it, so `Assistant(tool_call X)`, `User(U)`, `ToolResult(X)` is
representable too — and folding *that* result would move it before the user's
turn.

> **A committed tool result folds into its call's card only if every canonical
> fact it would be moved across belongs to the same foldable batch.**

Fold eligibility is therefore a property of the **complete canonical interval
between the call anchor and the result position**, not of the owning
assistant message's block tail. A batch — an assistant message's trailing
unbroken run of `tool_call` blocks — folds only when both hold:

```text
inside the message   nothing but tool_calls follows the batch's first call
across the interval  every entry from the anchor through the batch's last
                     committed result is a result of that same batch
```

Any `User`, `System`, or unrelated `Assistant` message in that interval
unfolds the batch. The decision is per batch and all-or-nothing: folding only
the calls whose results happen to be adjacent would move results across their
own siblings. The plan is derived fresh from the ordered transcript every
time — no `alreadyFolded` memory — so a resync agrees with itself.

When a batch does not fold, each of its calls is drawn as two fragments of one
entity:

```text
A

◇ Bash · result below
  $ cargo test

B

↳ ✓ Bash · ok · 2.8s · exit 0
  $ cargo test
  test result: ok. 842 passed
```

One identity, canonical order intact, and never the pre-#79 duplication of a
raw call block plus a running card plus a separate result block. Expanding
either fragment expands both: they are one card.

### Renderers may format, never decide

> **Tool identity may select a presentation renderer.
> Tool identity may never select or infer execution semantics.**

A stable `ToolId` picks a specialized renderer — Bash, Read, Grep, Glob, Edit,
Write — so a shell call reads as `$ cargo test --all` instead of argument JSON.
A renderer formats already-authoritative facts and is never handed the
lifecycle, so it cannot express an opinion about it: running, success, failure,
denial, cancellation, timeout, interruption, progress, duration, exit code, and
truncation all come from the Runtime Client. Nothing reads a status out of
output text, infers running from an absent result, or infers cancellation from
missing output. A renderer that does not recognise a shape returns nothing and
the generic renderer takes over, so unknown, MCP, and Python tools stay fully
usable.

### Visual collapse is not runtime truncation

> **Every externally-derived band of a collapsed card is finite in both line
> count and content length.**

One dimension is not a bound. `{"payload": "<100 kB>"}` is three
pretty-printed lines; a 50 kB path, Grep pattern, or Bash command is one line;
a 50 kB denial reason is one line. A "show the first 8 lines" rule prints all
of them in full. So every band carries a two-dimensional budget:

```text
header         glyph, title, runtime lifecycle    clipped, always visible
subject        the one-line identity of the call  bounded, always visible
call detail    argument JSON, a diff, a command   bounded when collapsed
reason         failure / denial prose             bounded when collapsed
result summary runtime-published counts           bounded, always visible
result detail  the body                           bounded when collapsed
truncation     the runtime's own TruncationState  always visible
```

The card shell owns the bound, not the renderers — a renderer never receives
the collapse context, so a huge MCP argument object, a large Edit diff, a
forty-line Bash command, and a partially streamed fragment are all bounded
without any renderer having to remember to do it, including renderers written
later, and including one that puts arbitrary prose in `summary`. The two
detail bands have separate budgets, so a verbose call never squeezes its
result off the screen. An elision marker names both dimensions when both
apply: `… 2 more lines · 49016 more characters · ctrl+o to expand`.

The status header names the settlement the runtime published — `failed`,
`denied` — and stops there. The runtime's prose explaining it appears once, in
the bounded reason band, because an always-visible header that no collapse can
shrink is the wrong home for an unbounded string. `cancelled (user_requested)`
keeps its reason: a `CancellationReason` is a small typed enum, not prose.

Expanding re-renders facts the client already holds: no re-execution, no
filesystem access, no network, no runtime request. The subject stays one line
either way; expanded, it is the complete published value.

### Collapse is finite *and* reversible

```text
client collapse    finite, and reversible from facts already held
runtime truncation authoritative, and irreversible
```

Every band the client bounds is a band the client can restore, because
restoring it spends nothing but `PresentationState`. That is what makes a
bound safe to apply to text a decision is made from. **One expansion state per
entity governs every expandable band of that entity** — a background card
never expands its result body while leaving its failure reason permanently
clipped, and a pending approval's runtime-published reason and validated
arguments are both revealed together.

The runtime's own `TruncationState` is the opposite kind of fact: those bytes
never reached the client. It is reported separately and always, and expanding
never undoes it.

### Pending approvals are bounded but never hidden

A 50 kB approval reason or a `Write` request carrying 50 kB of content is
collapsed by default, because an approval prompt that scrolls its own question
off the screen is one nobody can answer. `/expand interaction <id>` reveals
the complete reason and the complete validated arguments, rendered from the
interaction the client already holds — no runtime request, no re-execution, no
read.

This is disclosure, not a second approval gate. Nothing requires the card to
be opened before `/approve`, and expanding cannot edit what is being approved:
the arguments are drawn exactly as the runtime validated them, and the runtime
resumes the operation it already holds.

### Three identity domains, three preference sets

```text
ToolCallId       a logical model-issued tool call    foreground cards
ToolExecutionId  a detached background execution     background cards
InteractionId    one runtime-owned pending approval  interaction cards
```

All three serialize as transparent strings and nothing forbids the same string
appearing in all three, so expansion state is kept in **three** sets rather
than one string-keyed set. No naming convention (`call_*`, `exec_*`) is relied
on anywhere — a wire spelling is not a type.

```text
/expand                               toggle the latest tool call
/expand latest                        the same
/expand all                           expand every tool, background, and
                                      interaction card
/expand none                          collapse all three domains
/expand <tool-call-id>                toggle one foreground card
/expand background <exec-id>          toggle one background card
/expand interaction <interaction-id>  toggle one pending approval card
```

A bare id addresses the `ToolCallId` domain, always: there is no search across
the namespaces and no "first match wins". `latest` stays scoped to the
`ToolCallId` domain too — "the latest" across three unrelated identity domains
would name whichever entity a tie-break rule picked, not the one on screen.

### Configured, effective, and attempt-frozen are three model facts

```text
configured   what the session asks for            SessionModelView.configured
effective    what the runtime would actually use  SessionModelView.effective
attempt      what the running attempt froze       AttemptModelView.primary
```

All three can differ at once and the UI never loses one. When they coincide the
footer shows one bare model name; the moment any two differ every one of them
is labelled — `cfg A · eff B · attempt C` — and all three are undroppable, so a
narrow terminal wraps rather than omitting or truncating a model identity into
a different, shorter, wrong one. The selector labels rows `configured`,
`effective`, and `attempt` for the same reason, and uses the word `current`
only when there is exactly one thing it can mean.

Catalog metadata and live configuration are likewise never merged. A catalog
row states what a model *offers*, including the profile the catalog would fall
back to (`catalog default medium`). What the session asked for and what the
runtime resolved are separate lines (`configured reasoning`, `effective
reasoning`). A catalog default is never presented as current configuration, and
no reasoning scale is invented for a capable model that declares no profiles.

### Reasoning visibility is not reasoning configuration

The TUI consumes only canonical Runtime Client reasoning blocks. Provider
spellings such as `reasoning` and `reasoning_content` never enter the
TypeScript protocol or presentation layer. Whether reasoning is *drawn* is a
client preference (`/reasoning`, `ctrl+t`); when hidden it collapses to a
`Thinking…` marker rather than becoming assistant text. What rustX *asks a
provider for* is `SessionModelConfig.reasoningProfile` / `reasoningEnabled`,
which only `model_set` changes.

### Working status is proven, never timed

The spinner names a phase only when a projection fact proves it — a pending
interaction, a running or assembled foreground execution, the kind of the
latest streamed block, the attempt phase. There is no timer and no inactivity
threshold, and no state is invented for a lifecycle rustX does not publish.

## Testing

```sh
pnpm --dir tui typecheck
pnpm --dir tui test
```

Everything is deterministic: scripted byte and record sequences, a data
barrier rather than a delay for readiness, and no elapsed-time wait used to
establish rustX semantic ordering. The one isolated parser-boundary helper in
`app.test.ts` waits for pi-tui's bare-Esc disambiguation window; it does not
synchronize Runtime Client or Session behavior. The presentation suites drive
projection facts directly and assert on normalized strings — `transcript.test.ts`,
`tool-correlation.test.ts`, `tool-card.test.ts`, `model-selector.test.ts`,
`status.test.ts`, `identity-domains.test.ts`, and `reconstruction.test.ts`,
which rebuilds the whole visible UI from one fresh snapshot.

`test/integration.test.ts` drives the **real** `rustx` binary over the real
stdio/JSONL transport against a local SSE provider fixture (no credentials, no
network). It skips itself with a clear reason when `target/debug/rustx` has not
been built; set `RUSTX_BINARY` to point elsewhere.

## Licensing

See [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md).
`@earendil-works/pi-tui@0.82.1` is consumed as a published MIT dependency; no
Pi source is copied into this repository.
