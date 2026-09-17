# CFG3 local runtime example

This directory is a complete Workspace example. Its single `rustx.toml` declares
independent Providers/Models, runtime policies and explicit Root capabilities.
Resources live directly in `.agents`: Skills, named Agents, Workflows, MCP definitions
and an inert Python package. The endpoint and credential reference are placeholders.

```sh
cargo build --bins
./target/debug/rustx config check --config "$PWD/examples/local-runtime/rustx.toml" --workspace "$PWD/examples/local-runtime"
./target/debug/rustx config show --sources --config "$PWD/examples/local-runtime/rustx.toml" --workspace "$PWD/examples/local-runtime"
```

These commands inspect offline; provider readiness remains unresolved. To run, author
real Provider/Model facts or initialize your User configuration, then launch with
`--workspace /absolute/path/to/this/directory`. `--config` replaces only the User
source pathname and does not relocate User resources or runtime storage.

`minimal/rustx.toml` demonstrates the smallest complete model setup. The
`workflow-basic` and `workflow-templates` directories are independent Workspace
examples. See [the configuration reference](../../docs/configuration.md) for the full
schema, exact overlay matrix, resource shadowing, Plugins and admitted demand.

Saving a file does not mutate a loaded runtime. `/reload` publishes one complete new
generation. Restart always rereads current files. Default durable storage is
`~/rustx/runtime/sessions`, with one UUIDv7 Conversation directory and SQLite store
per lineage. Stable Tool output is retained beneath that Conversation.
