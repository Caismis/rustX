# Configuration commands

All commands use the [CFG3 native source resolver](configuration.md). They accept
`--workspace`, User-only `--config`, and startup `--runtime-root` bindings where
applicable. No command obtains configuration authority from resource existence.

| Command | Contract |
| --- | --- |
| `rustx init` | Create a strict User `rustx.toml` with explicitly supplied Provider/Model facts; refuse overwrite |
| `rustx config check` | Parse, overlay and validate prospective current configuration offline |
| `rustx config show --sources` | Show redacted prospective effective fields and native origins |
| `rustx config show --agent main` | Inspect Root profile and resource facts offline |
| `rustx workflow check <id>` | Inspect native Workflow validity and admission |
| `rustx workflow explain <id>` | Include the bounded native Workflow program projection |
| `rustx doctor --probe` | Print a finite side-effect plan, then probe admitted source demand |
| `rustx doctor --probe --prepare` | Also allow preparation for demanded Python packages |

Append `--json` for structured output. Exit 0 means the requested check completed
without unresolved facts; 2 means invalid arguments/configuration; 3 means incomplete
configuration or unresolved runtime readiness. Static inspection never resolves
execution credentials, connects providers/MCP, prepares Python, runs Workflows or
creates a Session. A declared provider remains connectivity-unresolved offline.

## Minimal initialization

```sh
rustx init --template openai-chat --provider service --model-id fast \
  --endpoint https://api.example.invalid/v1 --credential-env SERVICE_API_KEY \
  --context-window 128000 --max-output 4096 --tool-calls true --reasoning false \
  --compat 'chat_reasoning_replay = "omit"'
```

Substitute the explicit facts of your service. `init` creates the CFG3 User layout
and never generates split configuration files. It uses no-overwrite publication;
existing files are preserved. Use the [minimal example](../examples/local-runtime/minimal/rustx.toml)
for a complete authored document.

Diagnostics identify native source paths/fields with bounded correction guidance.
Unused invalid resources warn; selected invalid resources reject admission. Secret
values and unbounded malformed source bodies do not enter normal projections.
Prospective inspection never claims to describe a currently loaded generation.
