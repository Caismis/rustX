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
storage bindings remain fixed for that process. Reload never changes bindings.

`rustx app-server --listen stdio` owns one process and accepts Session creation
with explicit Workspace paths. WebSocket uses `--listen ws://IP:PORT` and a
`--token-file`. Process limits in User `app_server` require restart; Workspace
cannot author those process limits.

Root configuration and Session intent are distinct. `--model` is an explicit
Session choice. A cold resume reads current documents/resources and revalidates
that choice; it never restores materialized effective configuration. An omitted
Session choice follows the current Root default at the next eligible admission.

Save commits authored bytes. `/reload` publishes one complete immutable generation.
Restart rereads current bytes from scratch. None substitutes for another.
