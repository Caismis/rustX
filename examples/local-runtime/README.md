# Copyable local runtime configuration

This is the canonical example for the current explicit local runtime
contract. Copy this directory, replace the provider placeholder, and pass all
four paths explicitly. There is no cwd-based configuration discovery,
global/project precedence, implicit `~/.rustx` configuration, or TUI-side
configuration parser.

The intended layout is:

```text
examples/local-runtime/
├── models.json
├── session.json
├── workspace/
│   └── .agents/
│       └── tools/
│           └── echo/
│               ├── TOOL.toml
│               ├── input.schema.json
│               ├── pyproject.toml
│               ├── uv.lock
│               └── tool.py
└── .rustx/        # generated at runtime; normally absent before first run
```

## Who owns each path?

| Path | Owner and purpose |
| --- | --- |
| `models.json` | The runtime's provider/model authority: endpoint, credential source, model limits, capabilities, opaque request parameters, reasoning profiles, and protocol compatibility. |
| `session.json` | One conversation's initial model selection and overrides, context policy, native-tool policies, MCP bindings, and authorized environment. |
| `workspace/` | The model-visible working tree, including Skills and editable custom Python tool packages. |
| `workspace/.agents/tools/*` | Automatically discovered custom Python tool source packages; there is no separate registration entry in `session.json`. |
| `.rustx/` | Runtime-private generated artifacts, immutable Python tool versions, and environments. Keep it disjoint from `workspace/` and do not hand-edit it. |

The model works on files under `workspace/`. The Rust runtime owns private
state under `runtime-root` (the example uses `examples/local-runtime/.rustx`),
not inside the model-visible workspace.

## `models.json`

The provider identity `example` is only a local name. The explicit
`https://api.example.invalid/v1` endpoint must be replaced with the real
provider endpoint; a provider name never selects an official URL. The
credential is an environment reference, not a committed secret:

```sh
export RUSTX_EXAMPLE_API_KEY='replace-me'
```

The catalog demonstrates the current `openai_chat_completions` protocol, a
`example/demo-model` model with structured text/tool/reasoning capabilities,
model limits, and `compat` metadata. A model ID may contain `/`, so a model
such as `Qwen/Qwen3` is referenced as `example/Qwen/Qwen3` in the session.
`requestParams.temperature` is a
model-level provider wire parameter. It is opaque to rustX and is not a
universal sampling-parameter schema; adapt it to the selected provider.

`compat.chatReasoningReplay` selects the assistant-history spelling required
by the concrete OpenAI-compatible service:

| Value | Typical services | Behavior |
| --- | --- | --- |
| `reasoning` | vLLM, OpenRouter plaintext reasoning | Replays prior reasoning as `message.reasoning`. |
| `reasoning_content` | DeepSeek V4 thinking/tool turns, GLM preserved thinking, Qwen preserved thinking | Replays prior reasoning as `message.reasoning_content`. |
| `omit` | Legacy `deepseek-reasoner` ordinary multi-turn conversations | Does not send prior reasoning. |

This must follow the selected model's documentation; rustX deliberately does
not infer it from the provider name, hostname, or model ID.

There is no universal default: an `openai_chat_completions` model must declare
this field explicitly. These values control replay of historical assistant
reasoning only; they do not enable reasoning for the next generation.
Generation-time reasoning remains owned by the selected reasoning profile and
its provider request parameters. In particular, the `off` profile does not
implicitly select `omit`.

The named `off` and `on` reasoning profiles likewise have no built-in meaning
from their names. Their exact `enabled` state and provider-owned
`requestParams` are the contract. The illustrative `reasoning_effort` value
must be changed if the real provider uses a different reasoning parameter.

## `session.json`

The baseline session selects `example/demo-model` using the canonical
`provider/model` identity. `models.json` supplies the available model and its
defaults; `session.json.model` chooses this conversation's starting model and
overrides its `temperature` and output budget. The baseline uses the simpler
`summaryModel.mode = "session"` policy, so summaries follow the admitted
attempt's primary model.

`context` contains only session policy values (`reserveTokens`,
`keepRecentTokens`, and `summaryOutputCap`). The selected model's
`contextWindow` remains in `models.json`.

`nativeTools` shows the two policy axes for `read`, `write`, `edit`, `glob`,
`grep`, and `bash`: `execution` is one of `foreground_only`,
`background_only`, or `model_selectable`, and `concurrency` is one of
`sequential` or `parallel`. The runtime-intrinsic `background_task` tool is
not configured in this table; its fixed policy comes from the runtime.

The harmless `RUSTX_EXAMPLE_MODE` entry demonstrates the authorized session
environment. Keep provider credentials in `models.json`'s `apiKey` reference,
not in this table.

## MCP servers

The copyable baseline intentionally keeps `"mcpServers": {}`, so it does not
require an external MCP process or endpoint at startup.

`mcpServers` is a named map keyed by MCP server identity — the same shape
mainstream MCP clients use, so an entry can be copied straight from a
server's own documentation. Three canonical entries:

```json
{
  "mcpServers": {
    "exa": {
      "type": "http",
      "url": "https://mcp.exa.ai/mcp"
    }
  }
}
```

```json
{
  "mcpServers": {
    "exa": {
      "type": "http",
      "url": "https://mcp.exa.ai/mcp",
      "headers": {
        "x-api-key": "YOUR_EXA_API_KEY"
      }
    }
  }
}
```

```json
{
  "mcpServers": {
    "exa": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "exa-mcp-server"],
      "env": {
        "EXA_API_KEY": "YOUR_EXA_API_KEY"
      }
    }
  }
}
```

A stdio entry may also set `"cwd"`, which stays workspace-relative.

**Accepted shorthand.** Two shorthand forms from the ecosystem's own READMEs
are accepted and normalize to exactly the canonical entries above:

```text
url only     -> http
command only -> stdio
```

Documented and generated rustX configuration always uses the explicit `type`.
Nothing else is accepted: there is no `streamable-http`/`streamable_http`
alias, no `sse`, and no `ws`. An entry that is ambiguous or contradictory
(`url` together with `command`, `type: "http"` with `args`, a blank `url` or
`command`, an unknown field) fails startup rather than being guessed at.

**Tool policy.** rustX's own invocation policy for a server's tools lives in
the separate `mcpToolPolicies` map, keyed by the same identity, so an
`mcpServers` entry stays ordinary MCP configuration:

```json
{
  "mcpToolPolicies": {
    "exa": { "execution": "foreground_only", "concurrency": "parallel" }
  }
}
```

A server without an entry gets the deterministic default (`foreground_only`
and `sequential`). An entry naming a server `mcpServers` does not declare
fails startup.

rustX connects on whichever MCP protocol revision it and the server actually
share, so there is no protocol-version setting here.

After successful capability preparation, MCP tools are part of the
runtime-owned immutable capability snapshot. The TUI does not launch,
interpret, or configure MCP independently.

## Custom Python tool

The `echo` tool is discovered automatically from:

```text
<workspace>/.agents/tools/echo/
```

Its `TOOL.toml` uses the current manifest fields and
`foreground_only`/`sequential` policies. `input.schema.json` requires one
string `message`, and `tool.py` exposes the executor entrypoint
`def main(arguments)` and returns a JSON-serializable object. `pyproject.toml`
has no third-party dependencies, and the committed `uv.lock` is generated
from that project.

The runtime expects the `uv` executable to be available on `PATH` when it
prepares the discovered Python tool's private environment.

The package directory is editable source. During capability preparation,
rustX publishes immutable package versions and private Python environments
under `runtime-root/environments/`; those generated files are runtime state,
not workspace source and must not be hand-edited. Do not put a generated
virtual environment in `workspace/`.

## Run it from the repository root

The Rust binary speaks Runtime Client JSONL on stdout, so a human normally
uses it through `rustx-tui`.

```sh
export RUSTX_EXAMPLE_API_KEY='replace-me'

cargo build --bin rustx

./target/debug/rustx \
  --models ./examples/local-runtime/models.json \
  --session ./examples/local-runtime/session.json \
  --workspace ./examples/local-runtime/workspace \
  --runtime-root ./examples/local-runtime/.rustx
```

The endpoint in `models.json` is an example URL, so replace it before making
a model request. The binary remains a runtime process until its input closes;
its stdout is reserved for protocol records and diagnostics go to stderr.

For the reference TUI, install its locked dependencies once and use the same
four runtime paths:

```sh
pnpm --dir tui install --frozen-lockfile

pnpm --dir tui start \
  --binary "$PWD/target/debug/rustx" \
  --models "$PWD/examples/local-runtime/models.json" \
  --session "$PWD/examples/local-runtime/session.json" \
  --workspace "$PWD/examples/local-runtime/workspace" \
  --runtime-root "$PWD/examples/local-runtime/.rustx"
```

The TUI passes the paths through unchanged; the Rust runtime remains the sole
owner of model, session, tool, capability, and MCP semantics.
