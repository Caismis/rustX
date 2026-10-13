# TUI and App Server configuration

The TUI is a projection/control client of App Server protocol v47. It does not parse
TOML, resolve overlays or discover resources. Generated contracts live in
[`protocol/app-server/v47.ts`](../protocol/app-server/v47.ts).

`/settings` reads User configuration even with zero Sessions. `/settings workspace
"/canonical/path"` selects a native Workspace source; `rescan` and `approval
policy|full_access|inherit` operate on that explicit target with source CAS.
`/settings repairs`, `/settings user repairs` and `/settings workspace repairs`
inspect native automatic repair receipts (#464) read-only — changes are listed
as applied, eligible but not applied, or possibly published with durability not
confirmed, never an uncertain publication as applied. The current Workspace of
`/settings repairs` and `/settings workspace repairs` is the one the focused
Session is natively bound to: its canonical durable cwd, read with
`session/summary`, which loads no runtime. It is never the TUI's Session-creation
defaults and never a path resolved on the TUI machine, which may not be the App
Server's. With no focused Session, `/settings repairs` shows the User source and
says there is no current Workspace, and `/settings workspace repairs` reports
that instead of guessing; `/settings workspace "<path>" repairs` always inspects
exactly the named Workspace. A command captures the App Server and the Session
when it starts: a later focus change does not retarget it, and a result from a
replaced App Server is refused, never shown. Inspection sends only
`session/summary` and `configuration/sourcesRead` — no source write,
reconciliation, Turn or model request.

The host routes `configuration/sourceRepairChanged` without a Session. Before
anything subscribes, the client keeps source repair notices apart from other
early notifications, coalesced to the latest observation per source — a
withdrawal included — so no amount of unrelated traffic crowds them out (the
server advances its per-source cursor as it sends one, so a dropped notice would
be lost for good). They are bounded to the User and 64 Workspaces, the least
recently observed Workspace going first; a notice dropped there never reached a
subscriber, so nothing it would have ordered can go stale.
The first subscriber drains that buffer before anything is dispatched to it
directly, so the handoff never delivers an older observation after a newer one,
and never drops or repeats one.

The repair status line belongs to the App Server authority this connection's
`initialize` negotiated (`AppServerClient.authorityId`, exposed read-only by the
host). Three facts stay apart. A source's current native observation is ordered
by the native ledger sequence a notice (`sequence`) or read (`repair_sequence`)
carries — never by receipt id, status or arrival — so an older read answered
after a newer notice, or a late notice after a fresher read, changes nothing,
and an observation at an equal sequence is the same observation. Announcements
are kept once per `(authority_id, receipt id)`. What the line presents is a
source's current receipt from its first announcement until the user
acknowledges it or a newer observation replaces or withdraws it. A newer
observation therefore always updates its source even when its receipt was
announced before: it becomes current without a second announcement and retires
whatever the source presented (R1 → R2 → R1 ends with R1 current and nothing
presented). An acknowledgement never changes what a source holds.

Notices are not history: a source whose receipt was withdrawn while the client
was away is never announced to its next connection. A reconnect to the same App
Server therefore keeps the line and then rereads, through `configuration/sourcesRead`
on that exact connection, each source the line still presents — each Workspace,
whose read also rereads the User — one read at a time, bounded by the line's
sources. When the User is presented and no Workspace read succeeded (a
Workspace deleted or no longer a valid source refuses its read), one User-only
read follows, so an unreadable Workspace never keeps the independent User
source from reconciling; after a successful Workspace read the User is not read
again. A read that fails proves no withdrawal — not of its Workspace, not of the
User: the entry stays until a later read or notice (`/settings repairs` reads
again), nothing is retried or polled, and the connection is unaffected. Results of a replaced connection — checked by host
instance, not authority — and anything after quitting change nothing. A
replacement process, or another server, retires the previous line — a warning
never carries into a process that does not hold it — and its sequences start a
new ordering domain; its receipts are its own, even with equal ids.

One retention policy bounds every per-source repair map (`retainSource`: at
most 64 Workspaces, the ledger's own bound, whether or not the User has an
entry, plus the User in its own slot; the least recently observed Workspace is
forgotten first, the User never, and re-observing a Workspace makes it the most
recent). Each owner applies it to its own state: the client to its
pre-subscription buffer, the host to its connection's observations, the status
line to what it presents, retiring a forgotten source's warning with it.
Announcements are bounded separately (256). All of it is in memory only.

Forgetting a source never resets its order. The host keeps one frontier — the
highest sequence at which it forgot any source — so whatever an absent source
held was current at or before it. Its source is admitted when it is observed
again at or after the frontier: a newer native change, or a fresh read, whose
sequence native takes after receiving the request and so is never lower than
anything the host observed before sending it. A notice of an absent source
ordered before the frontier is ambiguous — the late history of a forgotten
source, or the old receipt of a source this connection only now addresses,
which native delivers at the sequence it was recorded — so it is never applied;
the host recovers that source with one authoritative `configuration/sourcesRead`
instead, one read at a time, one queued entry per source (at most 64), read
once more if noticed again while its read is in flight, and never after the
connection closed. A read is never ambiguous: each Workspace read in flight
remembers the highest sequence at which its own source was forgotten while it
was in flight, and its answer is applied to an absent source only when it is at
least that new, so a held read can never resurrect a source a newer
observation withdrew and then forgot. The caller still receives that read as
the snapshot it is. Beyond the current sources, the host keeps that one
sequence, its in-flight reads (each removed when it settles), and the bounded
recovery queue — nothing per source ever seen. The status line orders only what
the host admitted, which is never older for a source than what it already
delivered — and a later connection to the same authority observes each source
no older than its predecessor delivered it — so the line forgetting a source of
its own lets nothing stale back in.
`/session settings` inspects Session selections, binding, pending candidate and
the runtime's published adoption eligibility (Runtime Client state, never part of
the application). `/session adopt` sends only the last inspected candidate and
expected binding to `session/adoptConfiguration`. `/model` uses `session/setModel`;
it never writes a configuration source. `/debug` owns low-level diagnostics.
The obsolete `/configuration` and `/permissions` commands have no aliases.
Ordinary Save transfers work to the existing native coordinator. Clients do not
classify cache impact or resource closures, retry refused adoption, or replay an
uncertain mutation.

A spawned App Server receives process-only `--config` and `--runtime-root` bindings.
User resources remain `~/rustx/.agents`. Remote connections use the server's bindings,
not client-local configuration. Reconnect reconstructs current authoritative state;
it does not replay Save, adoption or other prior side effects.

See [configuration](configuration.md) and [development](../DEVELOPMENT.md) for launch
commands, and [the protocol](app-server-protocol.md) for transport/attachment semantics.

## Durable Session lifecycle (v35)

`/resume` opens a durable Session and implicitly ensures a compatible runtime.
Closing a view only detaches. No manual unload command or ordinary residency
status exists. Branch switching requests `session/switchNode`; retirement is
manager-owned. Confirmed deletion supports the focused Session and disables
control submission until authoritative settlement. Success focuses an existing
Session or opens the empty selector, without automatically creating a Session.
Lost deletion responses are never replayed; reconnection inspects native state.

`session/summaryInvalidated` is part of the mandatory v35 vocabulary and is
decoded with its mandatory `catalog_changed` scope and routed by Session identity. The TUI
holds no cached Session summary — `/resume` reads the catalog afresh every time
it opens — so the notification is accepted and declined: it is never folded into
the Conversation projection and never treated as invalid protocol input.


## Native child conversation inspection

Ctrl+Up/Down selects a Subagent, Enter opens its read-only transcript, `i` opens
status details, and Esc returns to Main with the exact parent draft preserved.
The popup shares transcript rendering and keeps a single 32-entry page; PageUp
reads older and Home resumes newest reads. It polls only transcript authority,
never derives content from status/activity. Parent epochs plus child read
generations fence late reads and paging. Reconnect/resync remembers only the
selected AgentId and reconstructs through `agent/transcript` on the current
parent AttachmentTarget, displaying explicit unavailable history when needed.
No child Session/Composer or HITL owner is created. See the
[protocol contract](app-server-protocol.md#read-only-native-agent-conversations-v35).

## Job and Agent control

The TUI folds `snapshot.jobs`/`job_updated` and `snapshot.agents`/`agent_updated`
independently. Agent selection uses stable AgentId, never activation ID; Active →
Inactive → resumed retains the same row and child transcript. Details show parent
lineage, current/latest activation, frozen profile and child ConversationId.
`/send-message`, `/wait-agent` and `/interrupt-agent` invoke native owner operations
without status preflight. `/jobs`, `/job-status`, `/job-wait` and `/job-cancel`
control finite Jobs. `/cancel` addresses only the primary attempt. Job completion
remains proactive and child final reports remain canonical conversation content.
See [the lifecycle contract](jobs-and-agents.md) and [TUI commands](../tui/README.md).
Initial `--model` intent is passed as `session/create.settings.model`; without it,
creation omits the override and the native default owner decides. Attachment reads
that established state. TUI startup does not issue an initialization `setModel`;
later explicit model changes remain supported. See [#419](issue-419/ownership.md).

## Incremental projection (App Server v35)

The existing terminal presentation reducer consumes every native event and the new
`read_domains_updated` cut (decorated transcript, statistics, Todo, occupancy).
Ordinary settlement no longer rereads Session snapshots. Snapshot acquisition and
subscription repair are coalesced by attachment epoch; a numeric cursor gap stops
incremental continuation. Replay joins the new snapshot cursor, with at most three
reacquisitions if the native replay ring moves during recovery. Read failures remain
visible and never replay mutations. Exact Tool assembly uses native `arguments_json`.
The terminal-specific normalized model and rendering remain unchanged; sharing the
Web raw DTO fold would require a second state representation, so no speculative
TUI rewrite or generic package was introduced.

## Image Tool and Bash presentation (#412)

See [the image and Bash contract](image-reading.md) for effective capability
intersection, Attempt-frozen publication, managed image ownership, provider
transport, text-only history projection, and presentation-only Bash descriptions.
