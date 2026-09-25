# TUI and App Server configuration

The TUI is a projection/control client of App Server protocol 20. It does not parse
TOML, resolve overlays or discover resources. Generated contracts live in
[`protocol/app-server/v22.ts`](../protocol/app-server/v22.ts).

`/settings` reads User configuration even with zero Sessions. `/settings workspace
"/canonical/path"` selects a native Workspace source; `rescan` and `approval
policy|full_access|inherit` operate on that explicit target with source CAS.
`/session settings` inspects Session selections, binding, pending candidate and
native eligibility. `/session adopt` sends only the last inspected candidate and
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

## Durable Session lifecycle (v22)

`/resume` opens a durable Session and implicitly ensures a compatible runtime.
Closing a view only detaches. No manual unload command or ordinary residency
status exists. Branch switching requests `session/switchNode`; retirement is
manager-owned. Confirmed deletion supports the focused Session and disables
control submission until authoritative settlement. Success focuses an existing
Session or opens the empty selector, without automatically creating a Session.
Lost deletion responses are never replayed; reconnection inspects native state.

`session/summaryInvalidated` is part of the mandatory v22 vocabulary and is
decoded and routed by Session identity like any other notification. The TUI
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
selected SubagentId and reconstructs through `subagent/transcript` on the current
parent AttachmentTarget, displaying explicit unavailable history when needed.
No child Session/Composer or HITL owner is created. See the
[protocol contract](app-server-protocol.md#read-only-native-subagent-conversations-v22).
