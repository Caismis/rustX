# TUI and App Server configuration

The TUI is a projection/control client of App Server protocol 7. It does not parse
TOML, resolve overlays or discover resources. Generated contracts live in
[`protocol/app-server/v10.ts`](../protocol/app-server/v10.ts).

`/settings` renders native effective/source/provenance/generation facts. `/reload`
calls the single `configuration/reload` operation and reports success, failure or
busy without changing the meaning of Save. `/model` updates deliberate Session
model intent through revision-fenced native control; it never edits User or
Workspace defaults. Obsolete Workspace trust and Session Tool/Skill narrowing
controls are absent.

A spawned App Server receives process-only `--config` and `--runtime-root` bindings.
User resources remain `~/rustx/.agents`. Remote connections use the server's bindings,
not client-local configuration. Reconnect reconstructs current authoritative state;
it does not replay Save, Reload or other prior side effects.

See [configuration](configuration.md) and [development](../DEVELOPMENT.md) for launch
commands, and [the protocol](app-server-protocol.md) for transport/attachment semantics.

## Durable Session lifecycle (v10)

`/resume` opens a durable Session and implicitly ensures a compatible runtime.
Closing a view only detaches. No manual unload command or ordinary residency
status exists. Branch switching requests `session/switchNode`; retirement is
manager-owned. Confirmed deletion supports the focused Session and disables
control submission until authoritative settlement. Success focuses an existing
Session or opens the empty selector, without automatically creating a Session.
Lost deletion responses are never replayed; reconnection inspects native state.
