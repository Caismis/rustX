# Launch configuration and project trust

TOML authors rustX configuration and model catalogs. YAML authors fixed Workflow
programs. Named Agents use TOML profiles; Skills and project guidance use Markdown.
JSON is reserved for wire data,
generated schemas, and explicitly opaque provider-native data. TOML bytes enter
strict snake_case structs, typed authority/merge rules, then resolved native state.

`agent.extensions.goal.enabled` defaults to `false`. Enabling it for a root launch
composes GoalDomain, the stable `get_goal`/`create_goal`/`update_goal` Tool surface,
typed current context, controls and the round driver. Ordinary Tool selection
cannot filter these Tools. Disabling it preserves durable Goal records; re-enabling
restores state disarmed. [Goal extension](goal-extension.md) defines the scope and
control contract. Complete child profiles suppress scope-ineligible Goal with a diagnostic; dynamic overrides requesting it are refused.

See [canonical named Agent resources](subagent-resources.md) for schema 8
Agent files, discovery/admission, bounded roots, source provenance, and frozen
reload/child contracts.


The [configuration command contract](configuration-diagnostics.md) defines
`rustx init`, offline `config check`, prospective `config show --sources`, and
explicit `doctor --probe`, including output, exit codes and effect guarantees.

`local_runtime::launch::resolve` is the only ordinary launch-resolution boundary.
It takes `LaunchRequest` and path-only `HostEnvironment`. Its shared `analyze`
phase finds bounded document slots, checks syntax/authority, merges explicitly
present fields, applies domain defaults, validates launch semantics and compiles
authorized local resources. Runtime admission then requires real host trust before
capturing credentials and returning `ResolvedLaunch` with safe provenance. Native composition consumes that
value; it does not reopen model/settings files to resolve startup.

Provider adapters still translate protocols. The composition owner constructs
provider bindings, Sessions, capability resources and Context Engine wiring.
Context Engine validates context policy against the actual selected model window.
Agent Loop retains execution, cancellation, tool lifecycle and settlement.

## Locations and identity (Linux and macOS)

Both platforms use the same convention:

| Location | Exact policy |
| --- | --- |
| User configuration directory | `$XDG_CONFIG_HOME/rustx`, otherwise `$HOME/.config/rustx` |
| Optional user settings | `<user configuration directory>/settings.toml` |
| Host model catalog | `<user configuration directory>/models.toml`; user `models` or CLI `--models` may replace it |
| User state directory | `$XDG_STATE_HOME/rustx`, otherwise `$HOME/.local/state/rustx` |
| Trust membership | `<user state directory>/trust/<workspace identity>/` |
| Default runtime root | `<user state directory>/workspaces/<workspace identity>/` |
| Project settings | Exactly `<resolved workspace>/rustx.toml`, optionally replaced by `--config` |
| Automatic Skill sources | `global` = `$HOME/.agents/skills`; `workspace` = `<workspace>/.agents/skills`; selected by `[skills].sources` |

HOME and supplied XDG paths must be absolute. Path discovery does not capture
credential values. Runtime/probe admission captures credentials through the
credential owner; tests inject snapshots without modifying process-global environment. Missing
optional settings are empty layers. A discovered malformed or unknown-field
document fails. An explicitly selected missing `--config` fails. Configuration
and catalog reads are limited to 1 MiB each, and retain strict TOML syntax.

An explicit `--workspace` selects that existing directory. Otherwise the
canonical launch directory is walked upward until the nearest directory with
either `.git` (file or directory) or `rustx.toml`. That directory alone is the
workspace. The walk has a hard 128-directory limit; exceeding it requires
`--workspace`. If there is no marker, a non-Git workspace is the launch directory.
There is no ancestor overlay chain. A nearer nested project or repository wins;
an explicit workspace ignores those markers. Use a project marker or explicit
workspace to give different subdirectory launches in a non-Git tree one identity.

Workspace canonicalization follows symlinks and requires an existing directory.
Aliases of the same canonical directory have the same identity. Identity is the
full lowercase SHA-256 of the canonical path's Unix-native bytes, explicitly
obtained with `std::os::unix::ffi::OsStrExt::as_bytes()` on Linux/macOS. This is
the persistent hash-input contract, not `OsStr::as_encoded_bytes()` or a
Rust-internal encoding. There is no lossy UTF-8 conversion, case folding,
Unicode normalization, salt, truncation or language/runtime hash. No identity
encoding is defined for unsupported platforms. Separate Git
worktrees have separate canonical paths and state, regardless of shared Git
administrative directories. Moving a workspace changes its identity; rustX does
not migrate, remove, or adopt old state automatically.

Runtime roots must be disjoint from the workspace, including existing symlink
ancestors. `--runtime-root` overrides only runtime state, never the host trust
store. Explicit previous state paths remain usable when valid under this rule.

## Precedence and field ownership

`built-in defaults < user settings < trusted project settings < explicit CLI`
applies only where a layer has authority. Forbidden fields fail even when a
higher layer would override them. Unknown fields fail at every schema boundary.

| Field class | User | Trusted project | CLI | Merge |
| --- | --- | --- | --- | --- |
| Model/provider declarations, endpoint, protocol, limits, capabilities, credential source | Host catalog; `models` chooses path | Forbidden | `--models` selects host catalog | Catalog replacement; no provider inference |
| Agent model selection and request policy (`agent.model`) | Yes | Existing host model only | `--model provider/model` | Explicit model-policy members; CLI selects fresh model policy |
| `agent_id` | Yes | Yes | — | Scalar replacement |
| `approval_mode` | Yes | Forbidden | — | Host scalar replacement |
| `context`, `model_timeout_policy`, `tool_deadline_policy` | Yes | Yes | — | Explicit members of these finite records |
| `agent.tools`, `agent.disabled_skills`, `agent.agents`, `agent.workflows` | Yes | Yes | Tool selection flags | Each selected dimension replaces; names select admitted resources. `agent.skills` is named-Agent authoring and is rejected on the root whenever it is authored, including `skills = []`. |
| `skills.sources` | Yes | Yes | `--skill`, `--no-skills` | Launch-scoped automatic source selection; list replacement, empty list selects none. An unselected source is inert: its root is never validated, scanned, or diagnosed. Resource reload rescans and revalidates the resolved roots but never rereads this policy |
| `mcp_servers`, `environment` | Yes | Yes | — | Named entries replace whole entries; empty map clears |
| `native_tools`, `mcp_tool_policies` | Yes | Forbidden | — | Host-only whole named entries; empty map clears |
| `agent.extensions` | Yes | Yes | — | Complete dimension replacement; an empty table composes none |
| `subagents.max_concurrent` | Yes | Yes | — | Runtime child capacity; scalar replacement |
| `subagents.workflow` | Yes | Yes | — | Static Workflow Agent-node admission; list replacement |
| Runtime state root (`runtime_root`) | Yes | Forbidden | `--runtime-root` | Path replacement |
| Workspace identity | No settings authority | Forbidden | `--workspace` | Canonical root selection |
| Trust records/store, credential-store redirection | No settings authority | Forbidden | `--trust grant/revoke` only | Host-owned membership operation |
| Session selection/name | No | No | `--continue`, `--session`, `--node`, `--name` | Existing Session semantics |
| `schema_version` | Yes | Yes | — | Explicit replacement; current schema only |

`agent.agents` selects root delegation authority; a non-empty resolved selection
derives the `subagent` dispatcher. `subagents.workflow` separately admits named
Agents for static Workflow Agent nodes. `agent.workflows` selects which admitted
Workflows the root may invoke. Valid unavailable Agent/Profile capability
selections produce diagnostics and suppression; Workflow static validation
retains its own failure contract.

Project MCP/resource declarations are permission to use those project-authored
resources. They cannot replace the model provider catalog or its credentials.
The independent external-source activation gate belongs to CFG-02.

Project trust is not Tool approval authority. Projects cannot set `approval_mode`
or any `native_tools`/`mcp_tool_policies` object, including empty objects or only
execution/concurrency members. These complete approval-bearing objects are
host-only; no project policy members are currently permitted. Declarations fail
before merge, rather than being silently overwritten. User settings retain all
three invocation-policy axes and runtime-wide approval mode.

Partial documents preserve absence. An absent list/map leaves the preceding
value. An explicit empty list replaces with no entries; an empty named map
clears preceding entries. Nonempty named maps retain other names and replace
same-name entries entirely, including omitted members of that entry. Structured
records such as `context = {}` contain no overrides. No arbitrary recursive
semantic merge, permission union, tombstones, includes or profile inheritance
exists. TOML has no null literal. An omitted field inherits; explicit domain
choices reset an inherited optional setting:

| Field | Use an explicit value | Reset inherited value |
| --- | --- | --- |
| `agent.model.reasoning_profile` | `{ mode = "profile", name = "on" }` | `{ mode = "catalog_default" }` |
| `agent.model.max_output_tokens` | `{ mode = "limit", tokens = 2048 }` | `{ mode = "catalog_default" }` |
| `context.summary_output_cap` | `{ mode = "limit", tokens = 1024 }` | `{ mode = "model_limit" }` |
| `tool_deadline_policy.idle_liveness_ms` | `{ mode = "window", milliseconds = 5000 }` | `{ mode = "disabled" }` |
| `agent.extensions.agent_status.time.timezone` | `"Asia/Shanghai"` | `"UTC"` |

For `agent.extensions`, timezone is a member of a complete replacement:
author the desired enabled extensions and contributors together. Setting only
one nested field does not preserve the preceding extension composition.

Catalog defaults mean the catalog's reasoning profile or output limit. `model_limit`
removes the additional summary cap; `disabled` removes the idle watchdog, preserving
the hard deadline. A profile named `catalog_default` is still selectable through
`{ mode = "profile", name = "catalog_default" }`. Explicit summary-model selections
use the same reasoning/output vocabulary and replace the whole summary policy.

Provider parameters have one authoring form in catalogs, reasoning profiles,
primary model overlays, and explicit summary model overlays:

```toml
[agent.model]
model = "example/demo-model"

[agent.model.request_params]
temperature = 0.7
top_p = 0.95
chat_template_kwargs.enable_thinking = true
structured_outputs.choice = ["positive", "negative"]
documents = [{ title = "A", text = "..." }, { title = "B", text = "..." }]

[agent.model.request_params.provider]
order = ["provider-a", "provider-b"]
allow_fallbacks = true
```

Tables (including dotted keys and inline tables), arrays, strings, integers,
finite floats and booleans normalize once into opaque provider-native JSON.
Dates, times, datetimes, NaN and infinities fail with the exact parameter path.
TOML has no explicit JSON null: omission is not null, and there is no sentinel or
raw JSON escape hatch. Programmatic JSON-domain parameters retain null support.
Protected wire keys are checked by the existing model owner after normalization.
Catalog/reasoning/session overlays remain shallow: replacing a top-level object
replaces that whole object. Admission and frozen model state are unchanged.

CLI-relative paths use the original launch directory. Agent Skill selections
are names, not paths; canonical discovery and explicit CLI Skill paths establish
resource existence. MCP cwd and executable paths containing `/` use the selecting
document's directory.
Role identities resolve from the pinned canonical role roots; supplemental
`agents_md.files` resolve from the owning workspace or user Subagent root. A bare MCP command
is still an executable name. MCP cwd also remains subject to the existing
workspace constraint. Workflow IDs refer to the explicitly owned resource root
`<workspace>/.agents/workflows`. Native tool paths retain execution-cwd semantics.
No global process cwd change is used. `Origin` records the document and its base
or the captured CLI base; rebasing occurs before merging, so a later document
cannot reinterpret an earlier relative path. Provenance carries no values or
credentials.

`--config` replaces the one project slot. It neither changes workspace identity
nor bypasses trust or field authority. There is no legacy explicit-path mode.

### Project resource path authority

Every project-origin local resource path must resolve inside the canonical
trusted workspace. This applies to canonical Skill and Agent resources and
`agents_md.files`, MCP `cwd`, and MCP `command` when it contains `/`. Explicit configuration
paths use their document directory; role supplemental paths use the role resource boundary, but neither `..`, an absolute path,
nor a symlink can grant access to a different workspace/worktree. An external
`--config` permits inert parsing of that document, not activation of its
neighboring files. There are no implicit external-resource grants.

The resolver checks each project layer before merge and retains the original
path spellings as project-authorized paths alongside field provenance. Initial
composition rechecks their physical targets without reopening configuration.
Reload checks the newly parsed pinned document layers and rejects the entire
candidate on authority failure, keeping the previous generation authoritative.
A corrected document may remove the rejected resource. Existing targets and
existing ancestors of missing targets are canonicalized at these boundaries;
dangling symlinks fail. In-workspace symlinks remain permitted where the resource
domain allows them; Skills and Python packages retain stricter package-symlink
validation. No protection against an OS user racing individual syscalls is claimed.
The authority root remains the launch-canonical path: replacing the workspace
itself with a symlink does not transfer its existing trust to the new target.

Workspace-owned automatic `.agents/tools` and `.agents/skills` roots receive the
same containment check before resource preparation. Selected project instruction
files and discovered Workflow files are checked at their read boundaries.
User/CLI-origin explicit resource paths are host authority and are not subject
to project containment; existing domain validation still applies (including MCP
cwd rules). Ordinary native tool file arguments, shell/command argument strings,
and child-worktree overlay paths retain their execution-domain semantics. This
is automatic resource authority, not a general filesystem or executable sandbox.

Project-origin MCP bindings also retain the original workspace authority in
their frozen internal binding. Every connect/reconnect rechecks path-valued
command/cwd before spawning; admitted children retain that same root, not their
new worktree as a broader authority. The internal binding field cannot be set
by TOML. Managed Python interpreter paths are host-materialized execution
resources; project package roots are checked before materialization.

## Trust

Every workspace must be explicitly trusted before ordinary composition, even
when its current directory is empty. This prevents later file creation from
silently enabling project content. Grant and revoke work without settings,
models, credentials, Python, MCP, or a Session:

```sh
rustx --workspace /path/to/project --trust grant
rustx --workspace /path/to/project --trust revoke
```

The TUI forwards the same operations to Rust and exits after their completion:
`rustx-tui --binary /path/to/rustx --workspace /path/to/project --trust grant`.
Trust is membership in the host state directory, scoped to canonical workspace
identity. Grant creates that identity's empty directory atomically and is
idempotent; revoke removes only that membership directory and is idempotent.
Unrelated identities cannot lose updates through whole-store rewrites.
Projects cannot redirect the store, declare trust, or inherit another
worktree's decision. The trust store itself cannot live under the workspace.

Untrusted launch exits with an actionable grant command before model, child,
Workflow, MCP or Python/resource activation. Inert document inspection may
produce syntax/authority diagnostics first. Trust permits project-authored
configuration and resources; it does not grant OS sandboxing, tool approval,
business approval, provider credentials or external-source lifecycle approval.
Revoke affects subsequent launches; it does not cancel a running runtime or
change already-admitted attempts. Restart to apply a changed trust decision.
Project instructions load only from the resolved workspace, with first-match
precedence `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`.
Instructions from unrelated ancestor directories are not activated.

## Minimal start and defaults

Use `rustx init` with explicit model/provider declarations (see the
[minimal initialization contract](configuration-diagnostics.md#minimal-initialization)).
It creates only user `models.toml` and `settings.toml`, with no project file,
implicit capability guesses or raw keys. Manual authoring is also supported;
the [minimal catalog](../examples/local-runtime/minimal/models.toml) shows the
required declarations. User `settings.toml` needs only the selected reference:

```toml
[agent.model]
model = "example/demo-model"
```

After granting trust to the workspace, launch `rustx` there, or launch the TUI
with only `--binary /absolute/path/to/rustx`. No project config is required.
No single-model guessing occurs: absent selection and unqualified/unknown
references fail clearly even if the catalog contains only one model.

New domain defaults are `agent_id: "rustx"` and context
`reserve_tokens: 1024`, `keep_recent_tokens: 4096`, `summary_output_cap: 1024`.
These do not infer or change the selected model's context window. Existing
domain defaults remain authoritative: schema 8, approval `policy`, model
summary policy `session`, model-declared reasoning/output defaults, no request
parameter overrides, native policies from `NativeToolPoliciesDocument`, the
native default tool list (`execution`, `ask_user`, `read`, `write`, `edit`,
`glob`, `grep`, `bash`), empty Skills/MCP/environment and
Subagent/Workflow admission, and Subagent capacity 4. Read is an ordinary
default-enabled Tool. The `subagent` dispatcher is derived from non-empty resolved
`agent.agents`, never authored as an ordinary Tool selector. All main selection
filters are exact; see
[selection and native defaults](runtime-resources.md#exact-tool-authority-and-native-defaults).
Native-only startup requires no Python or MCP;
project `.agents/tools` packages remain optional discovered resources.

Model response-start/stream-idle deadlines remain 30,000/15,000 ms. Foreground
tool execution retains its 120,000 ms hard deadline and no idle-liveness window.
The explicit lower-priority root product profile layer composes the Agent Status
extension with its Time and Background contributors enabled and no configured
timezone, and the Todo extension. Todo is deliberately absent from the native
default *tool* list above: it is composed by `agent.extensions.todo`, and naming it
in `agent.tools.builtin` is a validation error.

## Native Agent Extensions

`agent.extensions` is the root placement of the shared **closed, launch-scoped** profile composition surface for
optional Agent augmentation:

```toml
[agent.extensions.agent_status]
enabled = true

[agent.extensions.agent_status.time]
enabled = true
timezone = "Asia/Shanghai"

[agent.extensions.agent_status.background]
enabled = true

[agent.extensions.todo]
enabled = true
```

A Native Agent Extension is optional Agent behavior or context that belongs to
one concrete Agent/Conversation composition. Agent Status was the first
extension migrated under this boundary; **Todo** is the second, and the first
that is stateful and contributes a model-facing Tool. Goal is the third,
root-only extension over durable revisioned state. The obsolete top-level `agentStatus`
field is removed outright: there is no alias, no fallback parse, no
deprecation warning, and no compatibility mode — an obsolete document fails the
ordinary strict-field boundary, naming the offending field.

The record is *closed*, not an open registry. Its members are typed Rust fields,
so an unknown extension name (`agent.extensions.future_goal`) and an unknown knob inside a
known extension (`agent.extensions.agent_status.future`, `agent.extensions.todo.future`) both
fail at launch exactly like any other unknown field. rustX deliberately provides no generic plugin or
runtime-hook system: there is no dynamic registration, no lifecycle trait, no
event-hook registry, no arbitrary model-request mutation, and no
JavaScript/TypeScript/WASM or third-party extension loading. An extension may
contribute behavior only through an existing native owner — the Tool Plane for
tools, Context Assembly for request-time context, `ConversationRuntime` for
runtime coordination, the Runtime Client for projection. Adding one means adding
a typed member and wiring it through its real owner.

### Tool selection and extension composition are separate authority planes

Extensions and ordinary tools are separate concepts, and since the Todo
migration an extension may also contribute a model-facing Tool. The model's
Tool set is therefore a composition of distinct owners:

```text
  ordinary selected Tool capabilities          agent.tools.builtin / --tools /
                                               --exclude-tools / tools.builtin
+ enabled extension-provided Tool surfaces     agent.extensions.<name>.enabled
+ already-admitted domain terminal protocols   Workflow output, ...
```

Neither plane filters the other:

- `agent.extensions` never selects, enables, or filters an *ordinary* execution
  capability. Composing Todo adds no `read`, `bash`, or MCP tool;
- ordinary Tool selection never adds or removes an *extension-provided* Tool.
  `--no-tools` selects zero ordinary capabilities and leaves an enabled Todo's
  `todo` Tool in place, and `--no-builtin-tools` removes ordinary built-ins
  rather than every Tool that happens to be implemented in Rust. The
  classification is semantic, not incidental to where the code lives.

A truly Tool-free model request therefore requires **both** no ordinary Tools
**and** no Tool-providing extension:

```toml
[agent.extensions.todo]
enabled = false
```

Symmetrically, an extension can never be switched on by naming its Tool.
`todo` is rejected — deterministically, with a diagnostic naming the extension
— in `agent.tools.builtin`, `--tools`, `--exclude-tools`, a named Subagent's
`tools.builtin`, a Workflow's admitted capability set, and every invocation
override that shares that vocabulary. There is no alias and no compatibility
parse.

### The Todo extension

```toml
[agent.extensions.todo]
enabled = true
```

`enabled` composes one coherent capability, or none of it:

| composed | not composed |
| --- | --- |
| the conversation-owned `ConversationTodoList` | no current list authority at all |
| the model-facing `todo` Tool | no current `todo` Tool |
| the bounded read-only Todo status presentation | no Todo contribution to Agent Status |
| the Runtime Client / TUI Todo projection | no active Todo panel for this runtime |

The explicit lower-priority root product profile enables Todo. An omitted
Extension in a complete profile is absent; named profiles have no hidden
extension defaults. See [Agent Profiles](agent-profiles.md).

Todo carries no contributor configuration. The list's bounds, transitions, and
dependency rules belong to the list itself, not to launch configuration.

Disabling Todo is a statement about *this runtime*, never about history. A
Todo-disabled launch composes no list and reads no Todo history, while every
`todo` ToolCall and ToolResult the conversation already committed stays exactly
where it is and stays renderable as transcript history. Re-enabling Todo later
reconstructs the latest accepted authoritative snapshot from that same
canonical evidence — by reading the newest committed result, never by replaying
mutations — so it produces no duplicate ToolResults and no duplicate events.

Todo and Agent Status are independent axes. All four combinations are
intentional: with Todo on and Agent Status off the `todo` Tool, the list, its
ToolResults and its recovery all work exactly as before and there is simply no
reminder; with Agent Status on and Todo off, Time and Background continue
normally and no Todo section is fabricated.

### Launch-scoped lifetime

> A running `ConversationRuntime` executes against the native extension
> composition frozen for that launch.

The document is read once, at composition, through the ordinary launch
resolver, and frozen into the composed runtime. `agent.extensions` is therefore not a
reload-owned field: an explicit resource reload republishes a whole new
`RuntimeResourceSnapshot` and still cannot install, remove, or reconfigure an
extension inside an already-composed runtime. Restart/resume is a *new* launch —
it resolves the current document through the same resolver and applies it — and
it rewrites no canonical Session history to match a changed extension set.

### Root and child compositions

> Root Agent extensions and named-Subagent extensions are independently
> authored compositions.

The root Agent's composition comes from this document. A named Subagent's comes
from its own canonical Agent TOML (see
[canonical named Agent resources](subagent-resources.md)). A role that
declares no `extensions` composes none, independently of the root's
configuration.

A child never *implicitly inherits* the root's set. Since Issue #258 the
invoking Agent's frozen root composition does reach the resolver, but strictly
as **delegation authority** for an explicit invocation override:

```text
role default composition ------------------> effective child composition
invocation override (explicit) ------------> effective child composition
invoking Agent's frozen root composition --> AUTHORITY ONLY
                                             (may this caller ask for it)
```

So a child composes an extension for exactly two reasons: its definition
authored it, or an entitled caller explicitly asked for it and every requested
contributor was covered. Only the effective authorized composition enters
`ResolvedSubagentSpec`.

The invoking generation freezes the child's effective extension set into
`ResolvedSubagentSpec` before process staging and durable ownership commit, and
extension settings participate in the role's semantic digest. The child process
materializes that frozen decision and never rereads `rustx.toml`, host or
project configuration, role files, or a later resource generation to
reinterpret which extensions it owns.

Disabling an extension composes an ordinary runtime with that behavior absent.
`"extensions": { "agentStatus": { "enabled": false } }` leaves the Agent Loop,
tool admission and selection, tool execution, cancellation, attempt settlement,
terminal events, canonical history, and provider-independent messages exactly
as they are with the extension present; the only difference is that no Agent
Status is composed or emitted. The same holds for `"todo": { "enabled": false }`:
the only differences are that the `todo` Tool is absent from the model's Tool
set and the conversation composes no task list.

A Todo-enabled child owns **its own** list. It belongs to the child
conversation and is rebuilt from that conversation's own canonical history,
which is empty at birth — so a child's list never aliases its parent's, two
concurrently running Todo-enabled children never observe or mutate each other's,
and no child snapshot merges upward. The child's final result remains the
existing bounded Subagent report or Workflow structured output; its list is
working state, not part of that result, and its internals do not enter parent
canonical history.

### Inspecting what an Agent is actually running with

This document is the **prospective** authority: `rustx config show --sources`
reports what a next launch would resolve from it, and from which authored
layer. It is deliberately not a report of the running Agent.

For the running Agent, `/settings` renders the frozen effective composition the
attached runtime projects — `RuntimeClientSnapshot.effective_extensions` — for
a root Agent and for a Subagent child alike. That projection is read from the
composition the runtime already materialized; asking for it opens no
configuration file and consults no resource generation. Editing this document
after launch therefore makes the two views disagree, and that disagreement is
the point: one describes the next launch, the other describes the Agent that is
running. See [effective settings and configuration
lifetimes](effective-settings.md) for the full ownership matrix.

## Session publication and frozen execution

Discovery never implies resume. Fresh/unused empty Session startup remains the
default; `--continue`, `--session` and valid `--node` selection remain explicit.
`LocalSessionProduct::compose` plans the destination, composes and recovers an
inactive core, binds its client host, and calls `commit_startup` once before
infallible activation. Failed resolution/validation cannot create Session state;
failed composition cannot publish another selection. Existing behavior may
leave an unpublished seed database after a composition failure; it is not a
selectable Session. Existing published catalog bytes remain authoritative.

`--inspect-conversation` resolves only workspace/state locations and uses the
read-only inspection owner. Unless `--runtime-root` is explicit, it reads only
the user settings document's `runtime_root` member to locate state. It does not
load project settings or models, validate unrelated runtime settings, check
activation trust, compose a runtime, create a Session or publish a selection.

Explicit resource reload rereads only the launch-pinned user/project document
slots and resource roots through the existing resource-generation owner. It
does not rediscover a project, reload the model catalog, redirect state, or
change the launch's trust decision. Only resource-derived fields are used for
that new generation. Attempt admission pins `RuntimeResourceSnapshot` and model
state; request snapshots freeze rendered authority. Children receive
`ResolvedSubagentSpec`/`FrozenModelSpec` and selected materialization inputs,
including worktree selection, and never call launch discovery. Edits and later
reloads cannot mutate an admitted attempt or widen a child.

Removed contracts: the four-required-path parser and `LocalRuntimePaths`
startup API, mandatory configuration boilerplate for agent/context, implicit
ancestor instruction authority, and native-launch reads of legacy automatic
Skill roots. No old/new switch, fallback configuration mode or migration exists.

The separation of user/project scopes and explicit workspace trust follows
established terminal-agent practice; see the
[Claude Code scope documentation](https://code.claude.com/docs/en/settings) and
[workspace trust documentation](https://code.claude.com/docs/en/errors).
rustX deliberately has only the finite layers and fail-closed policy specified here.
External-source activation is separate from launch trust and Tool exposure.
The [source activation contract](source-activation.md) defines schema 8's
`mcp_servers.<name>.enabled` and host-only sensitive references. Managed Python existence comes from canonical package discovery.

MCP and Managed Python selection uses [the shared ToolSource contract](tool-source-selection.md).
Definition/enablement is not Agent exposure; offline discovery is inert, and
only admitted demand enters native source preparation.
