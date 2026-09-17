# CFG3 configuration

`rustx.toml` owns authored configuration. `.agents` owns resource definitions.
Defining a resource does not grant the Root Agent permission to use it. Rust
parses, validates, overlays and resolves these sources; clients present native
facts and keep unsaved drafts.

## Filesystem and process bindings

```text
~/rustx/
├── rustx.toml
├── .agents/
│   ├── mcp.toml
│   ├── skills/<name>/SKILL.md
│   ├── tools/<package>/server.py
│   │                  /requirements.txt
│   ├── agents/<name>.toml
│   └── workflows/<name>.yaml
└── runtime/
    └── sessions/
        ├── catalog.json
        └── ses_<uuid-v7>/
            └── conversations/
                └── conv_<uuid-v7>/
                    ├── conversation.sqlite
                    └── tool-output/
                        ├── results/result_<uuid-v7>.txt
                        └── tasks/exec_<uuid-v7>.output

<workspace>/
├── rustx.toml
└── .agents/
    ├── mcp.toml
    ├── skills/<name>/SKILL.md
    ├── tools/<package>/...
    ├── agents/<name>.toml
    └── workflows/<name>.yaml
```

There are exactly two ordinary scopes: User and Workspace. The Workspace is the
selected directory; ancestor configuration and resource directories do not
accumulate. Runtime files can also contain ownership locks, SQLite sidecars and
physical execution scratch beneath their native owner directories.

`--config /absolute/file.toml` replaces the User document binding for this
process. User resources stay at `~/rustx/.agents`; the Workspace document stays
at `<workspace>/rustx.toml`. `--runtime-root /absolute/directory` replaces only
the process runtime storage root. Neither path can change through reload.
Workspace identity never determines the default runtime storage root.

```sh
rustx --workspace /absolute/project
rustx --config /absolute/user.toml --runtime-root /absolute/runtime --workspace /absolute/project
rustx app-server --config /absolute/user.toml --runtime-root /absolute/runtime --listen stdio
rustx app-server --listen ws://127.0.0.1:7777 --token-file /absolute/server-token
rustx config check --workspace /absolute/project
rustx config show --sources --workspace /absolute/project
```

App Server WebSocket transport requires its dedicated token file. Listen
transport, authentication, process budgets and storage bindings live for the
process lifetime. Session creation supplies its Workspace independently.
`--model` and `/model` select deliberate Session model intent; they never write
Root defaults. `config check` and `config show --sources` describe prospective
composition from current files, not the published configuration of a loaded
runtime. They do not connect MCP or prepare Python. `doctor --probe` discloses
a finite probe plan; only selected demand is eligible for effects, and Python
preparation additionally requires `--prepare`.

## Typed document reference

The authoritative structural reference is
[`rustx.schema.json`](../schemas/rustx.schema.json), generated from
`local_runtime::authoring::RuntimeLayer`. Unknown fields are rejected. TOML
tables and arrays preserve authored omission versus explicit empty values.
The current document version is `schema_version = 9`.

| Top-level field | Owner and meaning |
| --- | --- |
| `schema_version` | Authoring format version; omitted uses the current format |
| `providers` | Independently named Provider definitions |
| `models` | Independently named Model definitions |
| `agent` | Root capability profile and model default |
| `agent_id` | Root runtime Agent identity label |
| `approval_mode` | Global `policy` or `full_access` execution approval mode |
| `context` | Context reserve, recent-history retention and summary cap |
| `model_timeout_policy` | Response-start and stream-idle deadlines |
| `tool_deadline_policy` | Tool hard deadline and optional idle-liveness window |
| `native_tools` | Global per-Native-Tool invocation policies |
| `mcp_tool_policies` | Global per-MCP-source invocation policies |
| `environment` | Literal Tool environment, keyed by variable |
| `subagents` | Runtime-global child capacity |
| `app_server` | User-only process policy; changes require restart |

### Providers and Models

Provider names and Model names are lookup identities. They do not infer protocol,
credentials, limits, endpoint, reasoning support or compatibility. A Model can
be replaced without replacing its Provider.

```toml
[providers.service]
base_url = "https://api.example.invalid/v1"
api_key = "$SERVICE_API_KEY"

[models.fast]
provider = "service"
id = "provider-wire-model-id"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096

[models.fast.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models.fast.compat]
chat_reasoning_replay = "omit"

[models.fast.request_params]
temperature = 0.2
vendor = { nested_option = true }

[agent.model]
model = "fast"
```

Each Provider requires `base_url` and `api_key`. Credentials are literal values
or explicit `$ENV_VAR` references. The environment is resolved by the credential
owner after admitted demand. Projections never return resolved secret values.
A Workspace Provider must supply its own complete definition, including its
credential source; no member is recovered from the shadowed User Provider.

Each Model requires `provider`, provider-native `id`, explicit `protocol`,
`context_window`, `max_output_tokens`, and the complete `capabilities` object.
Supported protocols and modality values are enumerated by the generated schema.
Optional `request_params` is an opaque, recursively structured native request
object. Optional `reasoning` declares `default_profile` and independently named
`profiles`, each containing `enabled` and optional native `request_params`.
Optional `compat` declares the adapter behavior: `chat_max_tokens_field`,
`chat_stream_usage`, `chat_reasoning_replay`, `chat_tool_protocol`, and
`responses_storage`. Protocol validation determines which members are applicable
and required. This boundary preserves provider-native parameters without allowing
them to replace runtime-owned request structure.

The Root `agent.model` object contains `model` and optional `request_params`,
`reasoning_profile`, `max_output_tokens`, and `summary_model`. A reasoning choice
is `{ mode = "catalog_default" }` or `{ mode = "profile", name = "..." }`.
An output limit is `{ mode = "catalog_default" }` or
`{ mode = "limit", tokens = 2048 }`. Summary selection is
`{ mode = "session" }` or `{ mode = "explicit", model = "...", ... }` with its
own reasoning, output and request settings. The whole selection is replaced
together. Domain defaults are evaluated inside that winning object.

### Root Agent

`agent` supports `description`, `instructions`, `model`, `tools`, `skills`,
`plugins`, `agents`, `workflows`, and `agents_md`.

```toml
[agent]
skills = ["review"]
agents = ["reviewer"]
workflows = []
instructions = "Explain the evidence for each proposed change."

[agent.tools]
builtin = ["read", "glob", "grep"]

[agent.tools.sources]
github = ["search"]
"python:analysis" = "all"
unused = []

[agent.plugins.todo]
enabled = true

[agent.agents_md]
inherit = true
files = []
```

Native Tools use an exact whitelist. An omitted whitelist grants none. MCP source
identities use their server names; Managed Python uses `python:<package>`.
Each source selection is `"all"`, an exact Tool-name array, or `[]` for none.
An empty source map replaces no named source entries; set a particular source
to `[]` to override its inherited selection. Agent and Workflow allowlists are
complete arrays, default empty. Their dispatcher capabilities follow admission
of those selections. Definition existence never selects a resource.

`agents_md.inherit` defaults true and `files` defaults empty inside its winning
object. Files are resolved by the native resource owner, captured with the
generation, and ordered as authored. Root has no child timeout or worktree
fields; those belong to named profiles.

### Plugins

Plugins are the closed Rust-owned `agent_status`, `todo`, and `goal` capabilities.
All default off. Enable one explicitly with `enabled = true`. Agent Status also
has `time` and `background` contributor objects. Time accepts its native timezone
setting. Replacing `agent_status` replaces its complete object, including both
contributors; a higher time setting never inherits the lower background setting.

Todo and Goal's current state lives in their Conversation domains, outside
configuration. Plugin composition controls capability availability. Named Agents
own their own supported Plugin composition; the native child scope rejects
Root-only Goal pursuit. No dynamic plugin loading, hook API or marketplace exists.

### Runtime policies

`context` has `reserve_tokens` (default 1024), `keep_recent_tokens` (4096), and
`summary_output_cap` (default 1024 tokens). The cap accepts
`{ mode = "model_limit" }` or `{ mode = "limit", tokens = N }`.
`model_timeout_policy` has positive `response_start_timeout_ms` and
`stream_idle_timeout_ms`. `tool_deadline_policy` has positive `hard_deadline_ms`
(default 120000) and `idle_liveness_ms`, either `{ mode = "disabled" }` or
`{ mode = "window", milliseconds = N }`. Meaningful progress can satisfy an
idle window but cannot extend a hard deadline.

`subagents.max_concurrent` is global child capacity, not a Root delegation
allowlist. `environment` maps variable names to literal strings.

Each `native_tools.<read|write|edit|glob|grep|bash>` object and each
`mcp_tool_policies.<source>` object has `execution` (`foreground_only`,
`background_only`, `model_selectable`), `concurrency` (`sequential`, `parallel`),
and `approval` (`never`, `always`). Native defaults are Tool-specific:
Read/Glob/Grep use foreground, parallel, never; Write/Edit use foreground,
sequential, always; Bash uses model-selectable, sequential, always. MCP policy
defaults are foreground, sequential, never. Omitted members of a replacement
object use these product defaults, never lower-source values. Python uses the
same invocation engine with native source policy; it has no separate authored
policy engine. Runtime control and Plugin Tools keep their domain-owned policies.

`app_server` accepts positive bounded `max_resident_runtimes` (8),
`max_connections` (32), `max_external_attachments` (64), `idle_grace_ms` (300000),
and `shutdown_deadline_ms` (30000). It is User-authored process policy. Workspace
authoring is rejected and reload cannot rebind the process.

## Exact overlay matrix

User is lower priority than Workspace. These are explicit typed replacement
units; there is no recursive TOML merge.

| Domain | Atomic replacement unit | Explicit empty behavior |
| --- | --- | --- |
| Independent scalars | `agent_id`, `approval_mode`, Root `description`, Root `instructions` individually | Empty strings undergo the field's validation |
| Providers | Complete `providers.<name>` | An incomplete Provider fails; `{}` map replaces no identities |
| Models | Complete `models.<name>` | An incomplete Model fails; `{}` map replaces no identities |
| Root model | Complete `agent.model` | Missing required model fails; optional members use domain defaults |
| Native selection | Complete `agent.tools.builtin` array | `[]` selects none |
| MCP selection | Complete `agent.tools.sources.<server>` value | `[]` selects none for that source |
| Python selection | Complete `agent.tools.sources."python:<package>"` value | `[]` selects none for that package |
| Skill visibility | Complete `agent.skills` value | `[]` exposes none; `"all"` exposes valid eligible packages |
| Plugins | Complete `agent.plugins.<plugin>` object | Empty object defaults off; omitted contributors use that Plugin's defaults |
| Agent delegation | Complete `agent.agents` array | `[]` permits no named Agents |
| Workflow admission | Complete `agent.workflows` array | `[]` permits no Workflows |
| Project guidance | Complete `agent.agents_md` object | `{}` uses inherit=true, files=[] |
| Native invocation policy | Complete `native_tools.<tool>` object | `{}` restores that Tool's product defaults |
| MCP invocation policy | Complete `mcp_tool_policies.<server>` object | `{}` restores domain defaults |
| Context | Complete `context` object | `{}` restores product defaults |
| Model timeouts | Complete `model_timeout_policy` object | `{}` restores product defaults |
| Tool deadlines | Complete `tool_deadline_policy` object | `{}` restores product defaults |
| Runtime child capacity | Complete `subagents` object | `{}` restores product defaults |
| Environment | One `environment.<variable>` string | Empty string is an authored empty value; `{}` replaces no identities |
| Process policy | Complete User `app_server` object | Defaults inside that object; Workspace rejected; restart required |
| Resource definitions | Complete same-name resource | Malformed higher resource still shadows lower resource |

Absence contributes no replacement. An empty identity map names no replacements
and never erases unrelated identities. Source deletion removes that source's
definition; prospective resolution can then select the other ordinary scope.
This is explicit removal, not fallback from an invalid higher definition.
Provenance records the winning object for its defaulted members as well as its
explicitly supplied members.

## Resources and admission

For every Skill, Agent, Workflow, Python package and MCP identity, the Workspace
definition shadows the complete User definition. Shadow selection occurs before
validation. An invalid Workspace duplicate never restores a valid User duplicate.
Unused invalid resources yield bounded ordered diagnostics; selecting an invalid
resource fails the typed resolution or admission operation. A malformed collection
whose identities cannot be read cannot expose lower definitions through fallback.

Skills use standard Agent Skills packages at `skills/<name>/SKILL.md`. `skills`
selection is prompt visibility, not a filesystem ACL. The system prompt provides
the resolved absolute User and Workspace Skill collection roots without listing
every absolute Skill path. Metadata supports progressive disclosure; bodies are
read through ordinary model-independent Tool behavior. A child receives captured
package bytes and resolved roots; it does not rediscover current files.

Named Agents use complete TOML profiles at `agents/<name>.toml` with description,
instructions, optional model selection, Native/source Tools, Skills, Plugins,
project guidance, optional `timeout_ms`, and `worktree` (`enabled`,
`require_clean_parent`). See [`agent.schema.json`](../schemas/agent.schema.json).
There is no Root Tool or Plugin ceiling. Root's explicit `agent.agents` array
controls whether delegation is allowed. Omitted child model means the invoking
Attempt's already-frozen effective model, including deliberate Session selection.
It never rereads the current Root default. Child admission freezes the complete
profile and generation. Existing child-scope restrictions remain domain-owned.

MCP definitions live only in `.agents/mcp.toml`:

```toml
[mcp_servers.github]
type = "stdio"
command = "github-mcp"
args = []

[mcp_servers.github.sensitive_env]
TOKEN = "$GITHUB_TOKEN"

[mcp_servers.search]
type = "http"
url = "https://example.invalid/mcp"
headers = { "X-Client" = "rustx" }
sensitive_headers = { Authorization = "$SEARCH_AUTHORIZATION" }
```

The bounded schema accepts `type` (`stdio` or `http`), `command`, `args`, literal
`env`, `sensitive_env`, `cwd`, `url`, literal `headers`, and `sensitive_headers`.
Transport-specific combinations are validated; a sole `command` or `url` can
identify its transport. There is no `enabled` activation field. Relative source
paths are resolved by their native definition owner. Secrets use explicit
environment references and never splice across resource shadowing.

Discovery captures definitions and inert Python package bytes. Root selection
produces finite Root demand. Merely listing a named Agent creates no demand for
that Agent's MCP/Python sources. Child demand begins at child admission and
preparation. Workflow demand follows its admission contract. The existing source
lifecycle owner resolves credentials, connects MCP or prepares Python, then
validates exact model-facing capability exposure. Failed required preparation
rejects the candidate. Unselected sources cause zero connections or preparation.
See [Workflow authoring](workflow-authoring.md) for the unchanged Workflow language.

## Save, reload and restart

Save is a source CAS write. Each typed mutation supplies the exact source
revision. The native writer takes the persistent document lock, checks that
revision, applies one semantic-unit mutation, validates and deterministically
serializes the complete rustX-owned TOML document, stages and syncs it, rechecks
the revision, renames into place, and syncs its parent. Comments are canonicalized
away. Competing cooperating writers have one winner. An external editor's changed
bytes invalidate a stale draft. Secret replacement/retention is scoped to the
same authored source, never a shadowed Provider.

An uncertain write response is repaired by rereading the authoritative source.
Clients preserve their draft and never blindly replay a mutation. Successful Save
does not update a loaded runtime. Its source revisions differ from the published
generation and the native projection reports pending reload.

`/reload` calls the single `configuration/reload` operation. The runtime builds a
candidate off-side: reread both documents and both resource roots; parse; apply
typed overlays; shadow resources; resolve models, policies and profiles; calculate
finite demand; prepare required external sources; validate the complete candidate.
Source revision fences reject changes during capture. The capability coordinator
owns one immutable `RuntimeResourceSnapshot` carrying `RuntimeConfiguration`,
catalogs, prepared sources, Tool registry and projection facts.

Publication replaces the coordinator's complete current resource snapshot under
its state lock through the runtime's private publication authority. That swap is
the configuration publication linearization point. The generation advances once
for a published candidate. No client can observe a new Provider with an old Root
profile. Active Attempts, background work, children, Workflows, maintenance and
competing reload ownership can return typed busy. Failure or cancellation before
publication leaves the exact old snapshot authoritative. Saving or watching files
never publishes; there is no filesystem watcher.

Already admitted work retains its generation and effective model. Later eligible
admission observes the new generation. Explicit Session model intent survives
reload and is revalidated against the candidate catalog. Cold composition always
rereads current files and process bindings, whether or not a prior reload occurred.
Durable Session configuration contains only `cwd` and optional explicit `model`.
Session names, graph focus and graph metadata have separate Session owners;
materialized effective configuration is never durable configuration authority.

## Durable identity and retention

Session, Session Node, Conversation and Tool Execution identities are typed
`ses_`, `node_`, `conv_`, and `exec_` prefixed UUIDv7 values. They are validated at
public and durable boundaries. Publication and graph order use explicit ordinals
and timestamps, never UUID lexical order. Attempt and interaction ordinals remain
where they have actual ordinal semantics.

Allocation checks native identity registries and filesystem absence and retains
no-overwrite creation. Deterministically injected collisions retry within bounded
allocation budgets or refuse; correctness does not assume a collision cannot
happen. Retired identities and burned reservations cannot be reused after restart.
Forking allocates a new Conversation without changing the source lineage identity.
Every Conversation has its own SQLite database, including multiple lineages within
one Session. There is no global or per-Session Conversation database.

Oversized foreground results preserve their bounded canonical ToolResult in
history and spill auxiliary continuation text to `results/result_<uuid-v7>.txt`.
Provider ToolCall IDs do not choose filenames and restart does not scan a numeric
watermark. Background acceptance and terminal/live output use the same
`tasks/exec_<uuid-v7>.output` locator. Stable model-readable output is Conversation
owned, outside OS temporary directories. Session/Conversation deletion owns its
cleanup; no unrelated TTL can invalidate a historical locator. Uncommitted child
rollback cleans its staged database and managed outputs while retaining its burned
identity reservation.

## Clients and format replacement

Web Settings has **Effective | User | Workspace** views. Effective is read-only
and shows native configuration, origins, selections, readiness and generation.
User and Workspace edit native structured drafts for Providers, Models, Root
selection, policies, Plugins, MCP definitions and complete named-Agent profiles.
The User config pathname and fixed User resource root are shown separately.
Save and Reload are distinct actions. Reload success shows the generation change;
busy or failure states that the old generation remains authoritative. CAS conflict
preserves the draft. Reconnect reads current state without replaying Save or Reload.

TUI `/settings` presents native effective/source/generation facts, `/reload` uses
the same native operation, and `/model` changes Session intent. Neither client
parses or merges configuration. App Server protocol 6 is generated from Rust; the
previous development protocol is rejected rather than dual-decoded.

CFG3 intentionally replaces the previous development configuration and durable
formats. There are no compatibility readers, aliases, trust commands, migration
shims or persisted-effective-config fallbacks. Old split configuration files are
not read. Incompatible durable schemas are refused. Author the current layout
explicitly; retain old development data separately if needed.
