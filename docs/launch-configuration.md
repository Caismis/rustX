# Filesystem and launch configuration

The complete [CFG3 reference](configuration.md) defines source bindings, the
schema and exact overlay units. Ordinary authoring uses `~/rustx/rustx.toml` and
`<workspace>/rustx.toml`; resource definitions use the corresponding `.agents`
roots. Workspace same-name resources shadow User resources completely, including
malformed duplicates. There is no ancestor configuration accumulation.

`rustx --workspace /absolute/project` resolves User < Workspace configuration.
`--config /absolute/source.toml` replaces only the User source pathname.
`--runtime-root /absolute/runtime` changes process storage, whose default is
`~/rustx/runtime`. Neither flag moves User resources. Configuration paths and
storage bindings remain fixed for that process. Application cannot change these bindings.

`rustx app-server --listen stdio` owns one process and accepts Session creation
with explicit Workspace paths. WebSocket uses `--listen ws://IP:PORT` and a
`--token-file`. User `app_server` admission limits apply automatically through native owners;
its process drain deadline requires restart. Workspace cannot author these limits.

Root defaults and resolved Session selection are distinct. Session creation pins
its resolved model, including when `--model` is omitted. Runtime unload/load within
the process restores its adopted binding. A new process resolves current sources
and preserves durable model selection. Global default edits affect new Sessions.

Save persists authored bytes and starts native reconciliation automatically.
Independent complete cache-preserving units apply to future Attempts; context
changes await explicit Session adoption. Rescan/retry uses `configuration/reconcile`.
Only actual process-lifetime bindings wait for restart.
