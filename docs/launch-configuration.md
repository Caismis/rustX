# Launch configuration and project trust

`local_runtime::launch::resolve` is the only ordinary launch-resolution boundary.
It takes `LaunchRequest` and a captured `HostEnvironment`, finds bounded document
slots, checks their syntax and authority, checks host trust, merges explicitly
present fields, applies domain defaults, validates launch semantics, and returns
`ResolvedLaunch` with safe field provenance. Native composition consumes that
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
| Optional user settings | `<user configuration directory>/settings.jsonc` |
| Host model catalog | `<user configuration directory>/models.jsonc`; user `models` or CLI `--models` may replace it |
| User state directory | `$XDG_STATE_HOME/rustx`, otherwise `$HOME/.local/state/rustx` |
| Trust membership | `<user state directory>/trust/<workspace identity>/` |
| Default runtime root | `<user state directory>/workspaces/<workspace identity>/` |
| Project settings | Exactly `<resolved workspace>/rustx.jsonc`, optionally replaced by `--config` |
| Automatic Skills | `<user configuration directory>/skills`, then `<workspace>/.agents/skills` |

HOME and supplied XDG paths must be absolute. Environment is captured once;
tests inject snapshots without modifying process-global environment. Missing
optional settings are empty layers. A discovered malformed or unknown-field
document fails. An explicitly selected missing `--config` fails. Configuration
and catalog reads are limited to 1 MiB each, and retain strict JSONC syntax.

An explicit `--workspace` selects that existing directory. Otherwise the
canonical launch directory is walked upward until the nearest directory with
either `.git` (file or directory) or `rustx.jsonc`. That directory alone is the
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
| Default Session model selection and request policy (`model`) | Yes | Existing host model only | `--model provider/model` | Explicit model-policy members; CLI selects fresh model policy |
| `agentId` | Yes | Yes | — | Scalar replacement |
| `approvalMode` | Yes | Forbidden | — | Host scalar replacement |
| `context`, `modelTimeoutPolicy`, `toolDeadlinePolicy`, `agentStatus` | Yes | Yes | — | Explicit members of these finite records |
| `defaultTools`, `skills` | Yes | Yes | `--tools`, `--exclude-tools`, `--skill`, disable flags | Lists replace; repeated CLI Skill paths form one replacing list |
| `mcpServers`, `environment` | Yes | Yes | — | Named entries replace whole entries; empty map clears |
| `nativeTools`, `mcpToolPolicies` | Yes | Forbidden | — | Host-only whole named entries; empty map clears |
| `subagents.maxConcurrent`, `.main`, `.workflow` | Yes | Yes | — | Scalar/list replacement |
| `subagents.definitions` | Yes | Yes | — | Same-name definitions replace whole entries; empty map clears |
| `workflows.definitions`, `.main` | Yes | Yes | — | Lists replace; YAML resources belong to workspace `.agents/workflows` |
| Runtime state root (`runtimeRoot`) | Yes | Forbidden | `--runtime-root` | Path replacement |
| Workspace identity | No settings authority | Forbidden | `--workspace` | Canonical root selection |
| Trust records/store, credential-store redirection | No settings authority | Forbidden | `--trust grant/revoke` only | Host-owned membership operation |
| Session selection/name | No | No | `--continue`, `--session`, `--node`, `--name` | Existing Session semantics |
| `schemaVersion` | Yes | Yes | — | Explicit replacement; current schema only |

Project MCP/resource declarations are permission to use those project-authored
resources. They cannot replace the model provider catalog or its credentials.
The independent external-source activation gate belongs to CFG-02.

Project trust is not Tool approval authority. Projects cannot set `approvalMode`
or any `nativeTools`/`mcpToolPolicies` object, including empty objects or only
execution/concurrency members. These complete approval-bearing objects are
host-only; no project policy members are currently permitted. Declarations fail
before merge, rather than being silently overwritten. User settings retain all
three invocation-policy axes and runtime-wide approval mode.

Partial documents preserve absence. An absent list/map leaves the preceding
value. An explicit empty list replaces with no entries; an empty named map
clears preceding entries. Nonempty named maps retain other names and replace
same-name entries entirely, including omitted members of that entry. Structured
records such as `context: {}` contain no overrides. No arbitrary recursive
semantic merge, permission union, tombstones, includes, profiles or inheritance
exists. Explicit null is accepted only by nullable domain fields (for example
`context.summaryOutputCap`); it is invalid for a list or map.

CLI-relative paths use the original launch directory. Config-relative Skills,
Subagent instruction files, explicit `agentsMd.files`, MCP cwd and executable
paths containing `/` use the selecting document's directory. A bare MCP command
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
trusted workspace. This applies to `skills`, Subagent `instructionsFile` and
`agentsMd.files`, MCP `cwd`, and MCP `command` when it contains `/`. Relative
paths still use their document directory, but neither `..`, an absolute path,
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
files and registered Workflow files are checked at their read boundaries.
User/CLI-origin explicit resource paths are host authority and are not subject
to project containment; existing domain validation still applies (including MCP
cwd rules). Ordinary native tool file arguments, shell/command argument strings,
and child-worktree overlay paths retain their execution-domain semantics. This
is automatic resource authority, not a general filesystem or executable sandbox.

Project-origin MCP bindings also retain the original workspace authority in
their frozen internal binding. Every connect/reconnect rechecks path-valued
command/cwd before spawning; admitted children retain that same root, not their
new worktree as a broader authority. The internal binding field cannot be set
by JSONC. Managed Python interpreter paths are host-materialized execution
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

Create the host `models.jsonc` manually with an explicit provider endpoint,
credential source, protocol, context window, output limit and capabilities
(see the [catalog example](../examples/local-runtime/models.jsonc)). Then put
only this in user `settings.jsonc`, using your declared model reference:

```jsonc
{"model": {"model": "example/demo-model"}}
```

After granting trust to the workspace, launch `rustx` there, or launch the TUI
with only `--binary /absolute/path/to/rustx`. No project config is required.
No single-model guessing occurs: absent selection and unqualified/unknown
references fail clearly even if the catalog contains only one model.

New domain defaults are `agentId: "rustx"` and context
`reserveTokens: 1024`, `keepRecentTokens: 4096`, `summaryOutputCap: 1024`.
These do not infer or change the selected model's context window. Existing
domain defaults remain authoritative: schema 6, approval `policy`, model
summary policy `session`, model-declared reasoning/output defaults, no request
parameter overrides, native policies from `NativeToolPoliciesDocument`, the
native default tool list (`execution`, `ask_user`, `read`, `write`, `edit`,
`glob`, `grep`, `bash`, `subagent`, `todo`), empty Skills/MCP/environment and
Subagent/Workflow admission, and Subagent capacity 4. Existing mandatory Read
semantics remain until CFG-03. Native-only startup requires no Python or MCP;
project `.agents/tools` packages remain optional discovered resources.

Model response-start/stream-idle deadlines remain 30,000/15,000 ms. Foreground
tool execution retains its 120,000 ms hard deadline and no idle-liveness window.
Agent Status time/background modules remain enabled, with no configured timezone.

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
the user settings document's `runtimeRoot` member to locate state. It does not
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
