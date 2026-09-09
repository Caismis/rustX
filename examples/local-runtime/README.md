# Copyable local runtime configuration

This is the advanced resource example for the Rust-owned local runtime.
Ordinary startup uses the [host configuration and trust contract](../../docs/launch-configuration.md).
Copy this directory and replace the provider placeholder to explore optional
resources. Rust owns discovery, defaults and trust; the TUI forwards intent.

The configuration files are commented in place. Read them first: this
document explains the contracts around them, while the files themselves
explain each field where it is set, and carry commented-out entries for the
options the baseline does not enable.

The intended layout is:

```text
examples/local-runtime/
├── README.md
├── models.jsonc
├── settings.jsonc
├── rustx.jsonc
├── workspace/
│   ├── AGENTS.md
│   ├── greeting.py
│   └── .agents/
│       ├── skills/
│       │   └── review-guidance/SKILL.md
│       ├── tools/
│       │   └── echo/{server.py,requirements.txt}
│       ├── subagents/
│       │   ├── navigator/{instructions.md,AGENTS.md}
│       │   ├── reviewer/{instructions.md,AGENTS.md}
│       │   ├── planner/instructions.md
│       │   └── implementer/instructions.md
│       └── workflows/
│           ├── parallel_review.yaml
│           └── implement_and_review.yaml
    # Runtime state defaults to the user state directory, outside this tree.
```

## Who owns each path?

| Path | Owner and purpose |
| --- | --- |
| `models.jsonc` | The runtime's provider/model authority: endpoint, credential source, model limits, capabilities, opaque request parameters, reasoning profiles, and protocol compatibility. |
| `settings.jsonc` | Copy to the host configuration directory: user model selection and host-only Tool approval/invocation policies. |
| `rustx.jsonc` | Project configuration: default model selection, context, Agent Status modules/timezone, tool activation, project MCP sources, contained Skill roots, and tool environment. It cannot set Tool approval/invocation policies. |
| `workspace/` | The authoritative execution cwd and conventional project/source tree, including Skills and editable custom Python tool packages. Relative native file-tool paths resolve here. This is not a general filesystem sandbox for Read/Write/Edit/Grep/Glob. |
| `workspace/.agents/skills/*` | Canonical project Skills, automatically discovered through the Skill plane's own semantics; a directory does not register a Workflow or Subagent. |
| `workspace/.agents/tools/*` | Inertly discovered managed Python packages. Explicit `pythonSources` enablement is required before preparing their ordinary MCP servers. |
| `workspace/.agents/subagents/*` | Explicitly defined/admitted Subagent instruction and project-guidance sources. The config controls admission; filesystem presence alone does not expose a profile. |
| `workspace/.agents/workflows/*` | Explicitly registered native Workflow YAML sources. The config controls both registration and model visibility; the directory is never scanned. |
| User state `workspaces/<identity>/` | Runtime-owned generated artifacts, prepared Python environments and Session storage, disjoint from `workspace/`. |

Native Read/Write/Edit/Grep/Glob paths may be relative to the execution cwd or
absolute host filesystem paths. `.` and `..` are resolved lexically before
filesystem or symlink behavior. Read/Grep/Glob may inspect an advertised
`ManagedToolOutput` path; that runtime-owned managed-output namespace is the
specific native-tool carve-out where model-originated Write/Edit is rejected.
The runtime root is not a general filesystem-security boundary. See the
[architecture](../../docs/architecture.md) and
[invariants](../../docs/invariants.md) documents for the complete contracts.

The runtime owns generated state under the resolved runtime root separately from the model's conventional
project tree. Project-authored Agent resources belong to the workspace-owned
`.agents/` namespace; `.rustx/` is not their canonical home. Native
file-tool paths are not implicitly confined to either tree.

## Configuration format

`models.jsonc` and `rustx.jsonc` are JSONC — ordinary JSON plus `//` and
`/* */` comments and trailing commas. Editors already understand the dialect
(`tsconfig.json`, VS Code settings); associating `*.jsonc` with "JSON with
Comments" is all the setup needed.

Nothing beyond comments and trailing commas is relaxed. Single-quoted
strings, unquoted property names, hexadecimal numbers, unary plus, missing
commas, and unknown fields all fail startup, because a configuration typo
must never parse into something the author did not write. A syntax failure
names the line and column; a schema failure names the field.

The relaxation is surface syntax only. Runtime-owned generated state under
`runtime-root` is unaffected and remains strict JSON.

## `models.jsonc`

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

`compat.chatToolProtocol` declares the model's *in-band* tool protocol, if it
has one:

| Value | Typical services | Behavior |
| --- | --- | --- |
| `native` (default) | OpenAI and most compatible services | Tool calls exist only as structured `tool_calls`; generated text is never inspected. |
| `qwen_xml` | Qwen tool models served by vLLM and compatible stacks | The reserved `<tool_call>` / `<function=…>` / `<parameter=…>` envelope is recognized. |

Some model families have a reserved tool syntax the serving stack is supposed
to parse out of generated text. When that parse fails, the reserved markup
leaks into ordinary content or reasoning and the request terminates as if the
model had simply answered. Declaring the dialect is what lets the adapter
report that as malformed tool intent instead of accepting it as an answer;
like every other `compat` value, it is never inferred from a provider name,
hostname, or model ID.

The named `off` and `on` reasoning profiles likewise have no built-in meaning
from their names. Their exact `enabled` state and provider-owned
`requestParams` are the contract. The illustrative `reasoning_effort` value
must be changed if the real provider uses a different reasoning parameter.

## `rustx.jsonc`

All project-origin local resource paths must resolve inside the selected
workspace, including symlink targets. This includes Skills, Subagent instruction
and `agentsMd.files`, and path-valued MCP command/cwd. `--config` only chooses
the project document; it grants no authority to its neighboring resources.
User/CLI resources retain separate host authority. Reload repeats containment
checks and rejects the whole candidate on failure.

The baseline runtime config selects `example/demo-model` using the canonical
`provider/model` identity. `models.jsonc` supplies the available model and its
defaults; `rustx.jsonc.model` supplies the default for a brand-new Session.
An existing Session's explicitly selected model is persisted separately in
the runtime-owned catalog and is never overwritten by this default.

`rustx.jsonc.model` chooses the starting model for a new Session and
overrides its `temperature` and output budget. The baseline uses the simpler
`summaryModel.mode = "session"` policy, so summaries follow the admitted
attempt's primary model.

`context` contains current runtime policy values (`reserveTokens`,
`keepRecentTokens`, and `summaryOutputCap`). The selected model's
`contextWindow` remains in `models.jsonc`.

`modelTimeoutPolicy` contains the two finite elapsed-time deadlines shared by
primary provider requests and compaction summary requests:
`responseStartTimeoutMs` defaults to 30 seconds and
`streamIdleTimeoutMs` defaults to 15 seconds. The policy is frozen for each
admitted request and is not part of Session history or the provider model
input.

Each named `subagents.definitions.<name>` may instead set an optional
`timeoutMs` execution deadline. It is a positive integer number of
milliseconds, bounded at 86,400,000 (24 hours); when omitted, no
definition-level deadline is installed. This limit covers the whole owned
child lifecycle — startup, model streaming, tools, and physical/workspace
settlement — and is separate from `modelTimeoutPolicy`. Expiration uses the
ordinary cancellation path with a deadline-specific reason; there is no
`TimedOut` subagent state. The model chooses only the named agent and its
task/context, and cannot set or extend this deadline per invocation.

`toolDeadlinePolicy` bounds each admitted foreground tool call:
`hardDeadlineMs` is the total execution lifetime (default 2 minutes) and the
optional `idleLivenessMs` cancels a started call that produces no progress
for that long. The idle window applies only to executors that declare
meaningful progress capability; executors without honest progress evidence
run under the hard deadline only. Executor progress refreshes only the idle
window, never the hard deadline. The policy is frozen at attempt admission,
and a deadline is cancellation intent: the loop cancels the call and awaits
the executor's physical settlement, bounded by a confirmation window —
committing `TimedOut` only when terminal settlement is proven, and
`OutcomeUnknown` when the executor cannot prove it.

Host `settings.jsonc` owns `approvalMode`, `nativeTools`, and `mcpToolPolicies`.
Project `rustx.jsonc` rejects all three objects, including execution/concurrency
members. Copy the optional policies from the host settings example if desired;
project trust never grants Tool approval authority.

`approvalMode` is the current runtime-wide HITL mode. It defaults to `policy`;
`full_access` suppresses only approval prompts for the current runtime and is
never restored from Session history. The runtime applies it at attempt
boundaries, so a busy attempt keeps its admitted mode while the latest request
waits as `pending`.

`nativeTools` can override three independent policy axes for `read`, `write`,
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
`properties` or `required`. Reference applicators (`$ref`, `$dynamicRef`,
`$recursiveRef`) are also refused at any depth, because rustX decorates the
root in place and a reference can re-enter it — inline the referenced
subschema instead. Apart from references, nested subschemas are unrestricted.

Rename the tool's field or flatten its root, or configure `foreground_only` or
`background_only`, which inject nothing and therefore accept any schema —
composed roots and `execution_mode` included. The same rule applies to
`mcpToolPolicies`, which is worth knowing before
switching an MCP server's tools to `model_selectable`: their schemas come from
the server verbatim, and a server that ships a composed root will be rejected
until you pick a fixed policy for it. Managed Python tool packages are exempt
from this decision: each package is served under the default
`foreground_only`/`sequential` policy, so its FastMCP-generated tool schemas
are never decorated. The runtime intrinsics `execution` and
`ask_user`, and the `todo` task list, are not configured in this table: all
three are fixed foreground, sequential, approval-never tools. `execution` is
the single model-facing control plane for conversation-owned asynchronous
executions (detached background tool executions and asynchronous subagent
children): call it with the typed execution handle (`kind` + `id`) a creation
result returned. `todo` keeps the
conversation's own task list — one call per change, and every settled call
returns the complete list, which is also what a restarted or resumed runtime
rebuilds the list from. `ask_user` accepts one structured
questionnaire object containing 1–4 related questions and publishes exactly one
questionnaire interaction through the runtime-owned `InteractionCoordinator`.
The client always offers bounded custom text; the model does not send
`allow_free_text` or author an `Other` option. A decline is a successful tool
result, while attempt cancellation and provider unavailability remain distinct.
The local process speaks Runtime Client protocol version 10, and its SQLite
conversation store accepts development schema version 18 only. Runtime Client
protocol versions superseded by the current one and development schemas before
version 18 are explicitly rejected rather than migrated.

The harmless `RUSTX_EXAMPLE_MODE` entry demonstrates the authorized runtime
environment. Keep provider credentials in `models.jsonc`'s `apiKey` reference,
not in this table.

`defaultTools` selects ordinary built-ins; Read is default-enabled.
`defaultTools: []` selects no ordinary built-ins, while explicitly admitted
main Workflows remain default-eligible. `--no-builtin-tools` removes all
built-ins, including generated dispatchers, from default selection.
`--tools a,b` is an exact allowlist; `--exclude-tools a,b` subtracts last.
`--no-tools` exposes zero ordinary main-model tools, including Read,
Subagent and Workflow tools. It conflicts with the other three selection
flags; `--tools` also conflicts with `--no-builtin-tools`.
Allowlist plus exclusions and default selection plus exclusions are supported.
Explicit lists reject empty entries/lists, duplicates, unknown/unavailable
names and ambiguous origins. Exclusions must resolve against applicable
availability, even if the identity was already omitted from selection.
Exposure controls do not disable enabled source preparation; use the source's
`enabled: false` for that. The TUI forwards these controls to Rust.

The product policy table is published in
[`settings.jsonc`](settings.jsonc) and
[runtime resources](../../docs/runtime-resources.md#exact-tool-authority-and-native-defaults).
The example inherits those defaults rather than demonstrating unusual overrides.
Partial native policy objects override only explicitly supplied axes over
that tool's own defaults. `full_access` bypasses only Tool approval, without
changing exposure, execution mode or concurrency.

Native launch discovers Skills under the user configuration directory's
`skills/` and `<workspace>/.agents/skills/`, plus the resolved `skills` list.
Repeating `--skill` supplies one list replacing the configured list.
This example uses only `workspace/.agents/skills/`.
`disable-model-invocation: true`
keeps a validated Skill in runtime resource state but omits it from the
model-visible catalog.

Lazy Skills are advertised only when native Read is admitted to that model's
frozen tool set. Removing Read hides the catalog, without discarding discovered
packages or eagerly inserting their bodies. Child Skill visibility follows
the child's independently frozen tool admission.

## Native YAML Workflows

Workflows are registered explicitly in `rustx.jsonc`; the runtime does not
discover every YAML file under the workspace. A registered id such as
`parallel_review` resolves exactly to:

```text
workspace/.agents/workflows/parallel_review.yaml
```

`workflows.definitions` is the registration set and `workflows.main` is the
independent model-visible set. Every id in `workflows.main` becomes one
concrete Tool named by that id, using the YAML `description` and `input`
schema. A registered-but-not-main workflow remains available to native
runtime composition but is not offered to the model. An unregistered YAML
file, even a malformed one, is irrelevant.

The YAML is serialization only. It deserializes into a `WorkflowDefinition`,
which is statically checked and compiled into an immutable `WorkflowProgram`;
`WorkflowRuntime` executes that program over the existing named
`SubagentRuntime`. The vocabulary is `Agent`, `Tool`, `Branch`, `Parallel`, `Review`, `Loop`,
and `Return`. Agent tasks are fixed strings, data movement uses
tagged references (`{type: reference, path: [args, task]}`), literals and
object/array constructions. Branch consumes a typed predicate. Root and
Parallel branches use identical lexical `block` graphs; a branch receives
explicit inputs and exports only its declared Return value. Parallel is
keyed and all-settle, including when native capacity requires waiting.
See [the scoped program grammar and limits](../../docs/workflow-programs.md).

Workflow Agent children complete successfully only through the reserved
`workflow_output` terminal protocol. It is not an ordinary Tool Plane call;
the frozen output schema is validated before an exactly-once commit. A turn
containing `workflow_output` must contain no ordinary tool call and no second
terminal call. Invalid or mixed turns perform no ordinary side effect and
receive bounded feedback so the child can continue. Workflow-local values and
child transcripts do not enter the parent conversation history.

Workflow subagent admission is independent from `subagents.main`: a profile
must be listed in `subagents.workflow` to be usable by a Workflow Agent, and
being main-visible does not grant Workflow admission. In this example
`navigator` is main-admitted while `reviewer`, `planner` and `implementer` are Workflow-only. A reload
constructs and validates the complete candidate, then publishes it atomically;
an active run keeps the immutable program snapshot with which it started.
Unfinished runs are not replayed after a crash.

## MCP servers

The copyable baseline intentionally keeps `"mcpServers": {}`, so it does not
require an external MCP process or endpoint at startup. `rustx.jsonc` carries
one http and one stdio entry commented out next to it; uncommenting one is
the whole edit.

`mcpServers` is a named map keyed by MCP server identity. Every entry needs
explicit `enabled: true` under host project trust. Credential-bearing entries
belong entirely in user settings, and replace as whole entries. See
[source activation](../../docs/source-activation.md). Three canonical entries:

```jsonc
{
  "mcpServers": {
    "exa": {
      "enabled": true,
      "type": "http",
      "url": "https://mcp.exa.ai/mcp"
    }
  }
}
```

```jsonc
{
  "mcpServers": {
    "exa": {
      "enabled": true,
      "type": "http",
      "url": "https://mcp.exa.ai/mcp",
      "sensitiveHeaders": {
        "x-api-key": "$RUSTX_EXA_API_KEY"
      }
    }
  }
}
```

```jsonc
{
  "mcpServers": {
    "exa": {
      "enabled": true,
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "exa-mcp-server"],
      "sensitiveEnv": {
        "EXA_API_KEY": "$RUSTX_EXA_API_KEY"
      }
    }
  }
}
```

A stdio entry may also set `"cwd"`, relative to its configuration document.
The resolved directory must still satisfy the MCP owner's workspace constraint.

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
the host settings `mcpToolPolicies` map, keyed by the same identity, so an
`mcpServers` entry stays ordinary MCP configuration:

```jsonc
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

The `echo` package is discovered inertly from its folder. The example disables
it; set `"pythonSources": {"python:echo": "enabled"}` to authorize preparation
in a trusted project. `--no-tools` does not disable this preparation.

```text
<workspace>/.agents/tools/echo/
├── server.py           # the FastMCP server; entrypoint is fixed: server.py:mcp
└── requirements.txt    # REQUIRED, even when empty
```

One folder is one package: rustX serves it as one MCP server whose identity is
`python:echo`, and every `@mcp.tool` function in `server.py` becomes one tool
of that server — a package may expose several tools. The tool schema is not a
separate file: FastMCP derives each tool's name, description, and input schema
from the decorated function's name, docstring, and type hints, so those are
the schema authority. The example declares one tool, `echo`, which echoes the
supplied `message`.

`requirements.txt` declares the package's own third-party dependencies, one
PEP 508 requirement per line (this example needs none). Do not declare
`fastmcp` itself: rustX injects and pins the managed FastMCP build, and a
package that declares it is rejected with a package-identifying diagnostic.

The runtime expects the `uv` executable (and a `python3` interpreter) to be
available on `PATH` when it prepares a discovered package's private
environment.

The package directory is editable source. During capability preparation rustX
fingerprints the package identity (the synthesized `python:<folder>` server
identity) together with the package bytes and the probed interpreter and uv
identities, and prepares one isolated uv environment per fingerprint under the
runtime-owned environment store (`runtime-root/environments/…/python-tools/`):
the frozen source copy, the generated `pyproject.toml` and `uv.lock`, the
`venv/`, and the manifest are all derived runtime state, never workspace
source, and must not be hand-edited. Do not put a generated virtual
environment in `workspace/`. The package folder name is part of the
environment identity: two distinct folders never share one prepared
environment, even when their files are byte-identical, and moving the whole
workspace to another host path does not change a package's identity. Editing
the package changes its fingerprint, so
an edit produces a new prepared environment that affects only future
capability activations; a running generation keeps the frozen server it
started with. The server process must keep stdout reserved for the MCP wire —
diagnostics belong on stderr; arbitrary stdout output is not a supported
logging channel. The `python:` MCP server namespace is reserved for these
discovered packages: a configured `mcpServers` entry may not declare a server
id starting with `python:` (rejected at startup with an actionable
diagnostic), so a package's synthesized identity can never collide with a
configured server.

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

First configure the host catalog at `$XDG_CONFIG_HOME/rustx/models.jsonc`, or
`$HOME/.config/rustx/models.jsonc` when XDG_CONFIG_HOME is unset. Use the adjacent
`models.jsonc` as a manual reference, replacing its endpoint and credentials.
The adjacent `settings.jsonc` is the minimal host model-selection example.
Ordinary startup needs no project file or path flags; this advanced example uses
`--config` to select its resource-rich project document outside `workspace/`.
That slot remains project authority and requires trust. See the complete
[launch/defaults/trust contract](../../docs/launch-configuration.md).

The Rust binary speaks the Runtime Client protocol over JSONL on stdout, so a human
normally uses it through `rustx-tui`. The reference TUI reconstructs pending
questionnaires from the authoritative snapshot, delegates custom-answer
editing (including bracketed paste and Unicode cursor behavior) to Pi-TUI's
native input component, and keeps the questionnaire's width- and
height-bounded viewport navigable on narrow and wide terminals.

```sh
export RUSTX_EXAMPLE_API_KEY='replace-me'

cargo build --bin rustx

./target/debug/rustx --workspace ./examples/local-runtime/workspace --trust grant

./target/debug/rustx \
  --config ./examples/local-runtime/rustx.jsonc \
  --workspace ./examples/local-runtime/workspace \
  --tools parallel_review,implement_and_review
```

The endpoint in `models.jsonc` is an example URL, so replace it before making
a model request. The binary remains a runtime process until its input closes;
its stdout is reserved for protocol records and diagnostics go to stderr.

For the reference TUI, install its locked dependencies once and forward the same
project intent to Rust:

```sh
pnpm --dir tui install --frozen-lockfile

pnpm --dir tui start \
  --binary "$PWD/target/debug/rustx" \
  --config "$PWD/examples/local-runtime/rustx.jsonc" \
  --workspace "$PWD/examples/local-runtime/workspace" \
  --tools parallel_review,implement_and_review
```

The TUI passes the paths through unchanged; the Rust runtime remains the sole
owner of model, session, tool, capability, and MCP semantics.

## Executable fixed Workflow references

Use `--tools parallel_review,implement_and_review` on the runtime/TUI launch
above to select exactly the two foreground Workflow Tools. This does not grant
leaf capabilities to the parent. Profiles and fixed Tool selectors have separate
explicit allowlists. Managed Python/MCP tools are unnecessary for these workflows.
The optional `echo` package is disabled by default and illustrates managed Python using the same MCP Tool Plane.

| Tool / YAML under `.agents/workflows/` | Input | Profiles / fixed Tools | Git | Nodes |
| --- | --- | --- | --- | --- |
| `parallel_review` | `{"proposal":"A concrete product proposal"}` | reviewer (`read` only) | No | Agent, Parallel, Branch, Return |
| `implement_and_review` | `{"requirement":"Keep the change minimal"}`; empty string requests a question | planner (no tools), implementer (`read`, `write`, `edit`, `ask_user`); fixed `ask_user`, `bash` | Yes | All seven |

Ask the parent to call the named Tool with these inputs. It makes one foreground
call; fixed child work and human interactions require no parent continuation.
For an interactive model, replace the placeholder endpoint, model id and credential
reference in `models.jsonc`, and the matching model references in `rustx.jsonc`.
Use a tool-capable Chat Completions provider. No paid service is needed for CI:
the provider emulator scripts ordinary model requests and native tools do the work.
Install Rust, the TUI's locked Node/pnpm dependencies, Git, `python3` and `uv` on PATH (the optional discovered echo package uses uv).
The checker uses only Python's standard library; it needs no Python Tool server.

For the implementation example, copy this directory outside any existing repository,
then initialize **the copied workspace**, keeping runtime-root outside it:

```sh
cd /path/to/copied/local-runtime/workspace
git init
git add .
git commit -m 'Initial example workspace'
```

This setup commit creates the clean baseline; the Workflow never commits or merges.
`greeting.py` deliberately fails the empty-name case. The trusted run binding acquires
one candidate at admission, before intake/plan Review, as required by the current
run-resource grammar. No implementation writer starts until the plan is accepted.
The planner uses the same isolated workspace policy with an empty tool allowlist.
Plan rejection disposes the unchanged candidate through native settlement.

The intake uses two predefined scoped branches: optional missing-information intake
and the fixed business contract. Questionnaire decline returns `unanswered` without
planning. The planner produces structured data, and Review accepts/rejects that exact
plan. The accepted path executes at most three implement/check bodies. Each implementer
exports its implemented plan anew; only those declared committed outputs and the actual
check report are carried forward. Private transcripts are never carried.

`Tool(bash)` executes a literal `python3 -I -B -` here-document in the candidate. The native
Bash JSON result supplies `stdout`, `stderr`, `combined`, and `exit_code`. A closed
`stdout` enum admits exactly `passed` or `failed`, then typed equality/Return produces
the boolean report. This is a fixed machine protocol, not log-keyword searching.
A zero exit with `failed` is a successfully executed check with business findings.
Missing files, exceptions, invalid return values, denial, cancellation, timeout and
unknown settlement never become `passed: false` and never enter another iteration.
`-I` excludes candidate import paths and Python environment overrides; `-B` prevents bytecode writes.
The verification program is frozen as trusted Workflow Tool arguments. The candidate
supplies the source under test, not the verifier implementation. Editing Workflow YAML
inside the candidate cannot mutate the already admitted program: execution uses the
frozen program generation. A candidate-side fake checker is only candidate data.
Tool Approval permits one exact invocation; it does not certify verifier trust. This
is not filesystem sandboxing against arbitrary external host processes.

Policy mode asks approval for each exact Bash, Write and Edit invocation. `full_access` bypasses that
permission only, never the fixed question or either human Review. An implementer's
explicitly allowed `ask_user` also routes to the root interaction surface, without a
parent relay. Questionnaire answers/decline, Review acceptance/rejection and Tool
Approval allow/deny are distinct responses with fresh instance identity.

Success at iteration N runs exactly N writers and N checks, then asks candidate Review.
Exhaustion returns `needs_revision` after three checks. Candidate rejection returns
`rejected`. These are completed business results, separate from native execution status.
The outer result includes native candidate identity and workspace handoff. Dirty bytes,
not just HEAD, identify the candidate. Later edits invalidate old check/Review
applicability. Inspect the retained path/diff; integrate it manually if appropriate.
No automatic commit, merge, stash, reset, deployment or new Workflow follows.

Every terminal path settles native borrowers first. Useful dirty work is retained on
acceptance, rejection, exhaustion, cancellation and failure; unchanged clean work can
be disposed. Unknown physical ownership is retained unresolved, not claimed safe to
write/delete. Cancellation is not rollback. The outer 30-minute deadline includes
human waits and all iterations; each native leaf keeps its ordinary deadline. Progress
and iterations do not extend either policy.

Parallel proposal review has no workspace binding and works in an ordinary non-Git
directory. Its quality branch has two fixed Agent steps; security has one. Each child
receives only its declared input. Definition keys (`quality`, `security`) determine
aggregation order regardless of completion order. Both settle before the deterministic
Branch and outer Return. A branch/body Return ends its own scope only.

See [the conformance map](../../docs/workflow-conformance.md) for ownership, deterministic
frontiers and tests, and [run details](../../docs/workflow-run-projection.md) for TUI
progress, resync and historical inspection. Live reconnect observes the same owner;
process restart never resumes old nodes or recreates actionable human decisions.
