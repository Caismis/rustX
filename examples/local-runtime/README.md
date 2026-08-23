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
├── rustx.json
├── workspace/
│   └── .agents/
│       └── tools/
│           └── echo/
│               ├── TOOL.toml
│               ├── input.schema.json
│               ├── pyproject.toml
│               ├── uv.lock
│               └── tool.py
└── .rustx/        # runtime-root; generated state, normally absent initially
```

## Who owns each path?

| Path | Owner and purpose |
| --- | --- |
| `models.json` | The runtime's provider/model authority: endpoint, credential source, model limits, capabilities, opaque request parameters, reasoning profiles, and protocol compatibility. |
| `rustx.json` | The current runtime/project configuration: default model for new Sessions, context/timezone, native-tool policy and activation, MCP sources, Skill roots, and authorized environment. |
| `workspace/` | The authoritative execution cwd and conventional project/source tree, including Skills and editable custom Python tool packages. Relative native file-tool paths resolve here. This is not a general filesystem sandbox for Read/Write/Edit/Grep/Glob. |
| `workspace/.agents/tools/*` | Automatically discovered custom Python tool source packages; there is no separate registration entry in `rustx.json`. |
| `.rustx/` (`runtime-root`) | Runtime-owned generated artifacts, immutable Python tool versions, environments, and Session storage. Keep it disjoint from `workspace/`; it is a separate ownership domain, not a promise that every file under it is unreachable through every filesystem mechanism. |

Native Read/Write/Edit/Grep/Glob paths may be relative to the execution cwd or
absolute host filesystem paths. `.` and `..` are resolved lexically before
filesystem or symlink behavior. Read/Grep/Glob may inspect an advertised
`ManagedToolOutput` path; that runtime-owned managed-output namespace is the
specific native-tool carve-out where model-originated Write/Edit is rejected.
The runtime root is not a general filesystem-security boundary. See the
[architecture](../../docs/architecture.md) and
[invariants](../../docs/invariants.md) documents for the complete contracts.

The runtime owns generated state under `runtime-root` (the example uses
`examples/local-runtime/.rustx`) separately from the model's conventional
project tree. Native file-tool paths are not implicitly confined to that tree.

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

## `rustx.json`

The baseline runtime config selects `example/demo-model` using the canonical
`provider/model` identity. `models.json` supplies the available model and its
defaults; `rustx.json.model` supplies the default for a brand-new Session.
An existing Session's explicitly selected model is persisted separately in
the runtime-owned catalog and is never overwritten by this default.

`rustx.json.model` chooses the starting model for a new Session and
overrides its `temperature` and output budget. The baseline uses the simpler
`summaryModel.mode = "session"` policy, so summaries follow the admitted
attempt's primary model.

`context` contains current runtime policy values (`reserveTokens`,
`keepRecentTokens`, and `summaryOutputCap`). The selected model's
`contextWindow` remains in `models.json`.

`approvalMode` is the current runtime-wide HITL mode. It defaults to `policy`;
`full_access` suppresses only approval prompts for the current runtime and is
never restored from Session history. The runtime applies it at attempt
boundaries, so a busy attempt keeps its admitted mode while the latest request
waits as `pending`.

`nativeTools` shows the three independent policy axes for `read`, `write`,
`edit`, `glob`, `grep`, and `bash`: `execution` is one of
`foreground_only`, `background_only`, or `model_selectable`; `concurrency` is
one of `sequential` or `parallel`; and `approval` is `never` or `always`.
Availability, activation, approval, approval mode, execution ownership, and
concurrency are separate facts.

Choosing `model_selectable` for a tool makes the model pick execution
ownership per call through a required top-level `execution_mode` field
(`"foreground"` or `"background"`) that rustX injects into the model-facing
schema, resolves once at preflight, and strips before the tool ever sees the
arguments. Because rustX writes into the root schema under that policy — and
only under that policy — a `model_selectable` tool's input schema must keep
its root simple: `type`, `properties`, `required`, and `additionalProperties`
describe the arguments, plus descriptive keywords like `title` and
`description`. Any other root keyword (`allOf`, `anyOf`, `oneOf`, `$ref`,
`maxProperties`, `const`, `enum`, `dependencies`, …) fails registration with
an explicit error, as does claiming the `execution_mode` name in root
`properties` or `required`. Nested subschemas are unrestricted.

Rename the tool's field or flatten its root, or configure `foreground_only` or
`background_only`, which inject nothing and therefore accept any schema —
composed roots and `execution_mode` included. The same rule applies to
`mcpToolPolicies` and to Python tool packages, which is worth knowing before
switching an MCP server's tools to `model_selectable`: their schemas come from
the server verbatim, and a server that ships a composed root will be rejected
until you pick a fixed policy for it. The runtime intrinsics `background_task` and
`ask_user` are not configured in this table: both are fixed foreground,
sequential, approval-never tools, with `ask_user` publishing one bounded
Question through the runtime-owned `InteractionCoordinator`.

The harmless `RUSTX_EXAMPLE_MODE` entry demonstrates the authorized runtime
environment. Keep provider credentials in `models.json`'s `apiKey` reference,
not in this table.

`defaultTools` controls native/built-in Tool activation. An empty list keeps
built-ins available for truthful capability inspection while activating none.
The command-line controls `--no-builtin-tools`, `--no-tools`, `--tools`, and
`--exclude-tools` apply after discovery; availability and activation are
separate runtime facts. The reference TUI accepts and forwards these controls,
as well as repeatable `--skill <path>` and `--no-skills`; it does not interpret
their values.

Skills are discovered from the current user/global and project roots, plus any
explicit `skills` paths in this file or repeatable `--skill` arguments.
`disable-model-invocation: true`
keeps a validated Skill in runtime resource state but omits it from the
model-visible catalog.

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
    "exa": {
      "execution": "foreground_only",
      "concurrency": "parallel",
      "approval": "never"
    }
  }
}
```

A server without an entry gets the deterministic default (`foreground_only`
and `sequential`). An `mcpToolPolicies` entry naming a server that
`mcpServers` does not declare is invalid and fails startup.

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
`foreground_only`/`sequential`/`never` execution, concurrency, and approval
policies. `input.schema.json` requires one string `message`, and `tool.py` exposes the executor entrypoint
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

## Native file-tool note

Read, Write, Edit, Grep, and Glob use `path`. Relative paths start at the
execution cwd established by `--workspace`; absolute paths are ordinary host
filesystem paths. Grep and Glob use the runtime's in-process
search implementation, and `.gitignore` behavior is unchanged. The
runtime-owned managed-output namespace remains readable/searchable through
supported paths but is not writable by model-originated Write/Edit.

The detailed limits, continuation diagnostics, and search semantics belong in
the [architecture](../../docs/architecture.md) and
[invariants](../../docs/invariants.md) documents rather than this copyable
configuration guide.

## Run it from the repository root

The Rust binary speaks Runtime Client JSONL on stdout, so a human normally
uses it through `rustx-tui`.

```sh
export RUSTX_EXAMPLE_API_KEY='replace-me'

cargo build --bin rustx

./target/debug/rustx \
  --models ./examples/local-runtime/models.json \
  --config ./examples/local-runtime/rustx.json \
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
  --config "$PWD/examples/local-runtime/rustx.json" \
  --workspace "$PWD/examples/local-runtime/workspace" \
  --runtime-root "$PWD/examples/local-runtime/.rustx"
```

The TUI passes the paths through unchanged; the Rust runtime remains the sole
owner of model, session, tool, capability, and MCP semantics.
