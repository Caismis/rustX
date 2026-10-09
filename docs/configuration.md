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
process (`rustx app-server` and the offline configuration commands). User resources stay at `~/rustx/.agents`; the Workspace document stays
at `<workspace>/rustx.toml`. `--runtime-root /absolute/directory` replaces only
the process runtime storage root. Both are fixed process bindings.
Workspace identity never determines the default runtime storage root.

```sh
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
The current document version is `schema_version = 10`.

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
| `environment` | Literal Tool environment, keyed by variable; projections carry the identities only |
| `subagents` | Runtime-global child capacity |
| `app_server` | User-only process policy; changes require restart |

### Providers and Models

Provider names, Model names and Profile names are lookup identities. They do not
infer protocol, credentials, limits, endpoint, reasoning support, compatibility
or any provider-native parameter. A Model can be replaced without replacing its
Provider.

Ownership is layered, and each layer owns only its own facts:

| Owner | Owns |
| --- | --- |
| Provider | `base_url` and the credential source |
| Model | provider binding, provider-native `id`, `protocol`, `context_window`, the hard maximum `max_output_tokens`, declared `capabilities`, `compat`, and either native `request_params` or named `profiles` |
| Model Profile | one complete, independent invocation preset: `reasoning_enabled`, an optional default `max_output_tokens`, and its own native `request_params` |
| Model selection | the chosen `model`, optional `profile`, explicit `request_params` overrides and an optional output limit |
| Provider adapter | protocol structure, the output-token field spelling, and final protected-key validation |

```toml
[providers.service]
base_url = "https://api.example.invalid/v1"
api_key = "$SERVICE_API_KEY"

# A Model without profiles: its request_params are the native default object.
[models.fast]
provider = "service"
id = "provider-wire-model-id"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
request_params = '{"temperature":0.2,"vendor":{"nested_option":true,"unset":null}}'

[models.fast.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models.fast.compat]
chat_reasoning_replay = "omit"

# A Model with profiles: each profile is a complete preset; the Model itself
# declares no request_params.
[models.thinker]
provider = "service"
id = "provider-reasoning-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 8192
default_profile = "balanced"

[models.thinker.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = true

[models.thinker.compat]
chat_reasoning_replay = "omit"

[models.thinker.profiles.fast]
reasoning_enabled = true
max_output_tokens = 2048
request_params = '{"reasoning_effort":"low"}'

[models.thinker.profiles.balanced]
reasoning_enabled = true
request_params = '''
{"reasoning_effort": "medium", "metadata": {"tier": null}}
'''

[models.thinker.profiles.precise]
reasoning_enabled = false
request_params = '{"temperature":0.1,"top_p":0.5,"stop":["\n\n"]}'

[agent.model]
model = "thinker"
profile = "fast"
request_params = '{"seed":7}'
```

Each Provider requires `base_url` and `api_key`. Credentials are literal values
or explicit `$ENV_VAR` references. The environment is resolved by the credential
owner after admitted demand. Projections never return resolved secret values.
A Workspace Provider must supply its own complete definition, including its
credential source; no member is recovered from the shadowed User Provider.

Each Model requires `provider`, provider-native `id`, explicit `protocol`,
`context_window`, `max_output_tokens`, and the complete `capabilities` object.
Supported protocols and modality values are enumerated by the generated schema.
Optional `compat` declares the adapter behavior: `chat_max_tokens_field`,
`chat_stream_usage`, `chat_reasoning_replay`, `chat_tool_protocol`, and
`responses_storage`. Protocol validation determines which members are applicable
and required.

#### Provider-native request parameters

Every `request_params` field in source TOML is a **JSON-encoded string** whose
JSON value must be one object. Nested objects, arrays, strings, numbers, booleans
and explicit `null` are preserved exactly; there is no provider-key catalogue.
Malformed JSON, a non-object root and a repeated key at any depth are rejected,
and a TOML table is never accepted. Diagnostics name the field (for example
`models.thinker.profiles.fast.request_params`), the error category and the line
and column of the JSON text — never an authored value or key, since a key may
itself hold a secret. The string is parsed once at the source boundary; App
Server and client JSON carry the parsed structured object. Native writes
re-encode it as compact JSON; formatting and key order are never semantics.

Numbers are held to the domain every hop reads alike — rustX, binary64 JSON
clients such as the browser, and the provider (the I-JSON rule of RFC 7493
§2.2): a number is accepted only when its value is exactly what its IEEE 754
binary64 reading prints back. `9007199254740993`, `2^60` and a decimal with more
significant digits than binary64 carries are rejected with a located diagnostic
instead of being silently rounded by a later hop, whether authored in TOML or
sent as a structured App Server value; the native writer can never emit one.
App Server requests are judged on their raw text: a JSON decoder rounds a
literal before any decoded value exists, so the server reads each literal
inside a declared `request_params`/`requestParams` member of the request schema
before decoding, and refuses a lossy one as Invalid params naming neither key
nor value.
`9007199254740992`, `0.1`, `1e300` and every other binary64-exact value
round-trip unchanged. A value that needs more precision belongs in a JSON
string if the provider accepts one.

#### Model Profiles

- A Model with `profiles` declares a nonempty collection and a `default_profile`
  naming one of them, and must not declare model-level `request_params` — not
  even `'{}'`.
- A Model without profiles must not declare `default_profile`; selecting any
  profile for it fails. A reasoning-capable Model without profiles keeps
  provider-default reasoning without a synthetic wire field.
- On a reasoning-capable Model every profile declares `reasoning_enabled`. On a
  non-reasoning Model omission means `false` and `true` is invalid. Reasoning
  state is never inferred from a profile name or a native key.
- A profile `max_output_tokens` is a positive default no greater than the
  Model's hard maximum.
- Profiles never inherit: neither from the Model nor from another profile.

#### Resolution

The effective native object has exactly one base and one shallow overlay:

```text
Model without profiles:  model request_params    + selection overrides
Model with profiles:     selected profile params + selection overrides
```

The overlay is top-level only: nested values are replaced atomically and a JSON
`null` is a real value, not a deletion. An override may add unrelated keys but
may not repeat a top-level key the selected profile declares, whatever its
value. That failure names the Model, the Profile and the override layer, never
the key: a provider-native key is authored content as opaque as a value.
Protocol-owned
fields — model identity, messages/input/instructions, tools, streaming, provider
continuation state and every output-token field — are protected in every layer
and again at final wire construction. The output budget is the explicit
selection limit, else the selected profile default, else the Model hard maximum;
no limit may exceed the hard maximum, and the Context Engine summary cap still
applies independently. An omitted `profile` selects `default_profile`; an
unknown or inapplicable explicit profile fails without fallback.

The Root `agent.model` object contains `model` and optional `profile`,
`request_params`, `max_output_tokens`, and `summary_model`. An output limit is
`{ mode = "catalog_default" }` or `{ mode = "limit", tokens = 2048 }`. Summary
selection is `{ mode = "session" }` or `{ mode = "explicit", model = "...", ... }`
with its own `profile`, output and request settings. The whole selection is
replaced together. Domain defaults are evaluated inside that winning object.
An admitted Attempt freezes the complete resolved invocation — Model, Profile,
reasoning state, output budget and parameters — so later configuration changes
affect only later admissions.

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
allowlist. `environment` maps variable names to literal strings in the authored
document. Those literals are secrets on the same terms as a Provider credential:
every projection that leaves native authority carries the identities alone, so a
client can discover, attribute and override an environment variable without ever
reading its value.

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
`read_image` has fixed foreground/parallel/approval-never policy; its selection
still uses `agent.tools.builtin`.

`app_server` accepts positive bounded `max_resident_runtimes` (8),
`max_connections` (32), `max_external_attachments` (64), `idle_grace_ms` (300000),
and `shutdown_deadline_ms` (30000). It is User-authored process policy. Workspace
authoring is rejected and application cannot rebind the process.

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

## Save, automatic application and Session adoption

`configuration/sourceWrite` is a typed source CAS operation. The native writer
checks the exact byte revision, validates and canonicalizes TOML, stages and syncs
it, checks again, renames it, and syncs its parent. External edits invalidate stale
drafts. Secret retention is scoped to the same authored source.

After persistence, native configuration coordination captures a finite immutable
input manifest and owns reconciliation independently of the RPC. Save requires no
second action. Saved source is desired state; it is not proof of effective state.
The application attempt identity distinguishes retries of the same input revision.
The source/application mutex orders commits against final candidate publication.

The finite units are execution policy, capability/resource closure, instructions,
provider/request construction, shared child capacity, and process bindings.
Independent approval and deadline policies apply to future independent Attempts.
Capability definitions, implementations, environments, MCP bindings and leases move
together. Candidate preparation happens off-side with one active preparation and
latest pending work per Session. Preparation has a bounded deadline. Failures do
not roll back independently applied units or mutate resources leased by old work.

Each Session retains its adopted binding across runtime unload/load and reconnect
within an App Server process. Comparison uses that Session's actual adopted
provider request shape. Adapter-built system contributions, ordered Tool schemas,
provider/model namespace and request parameters determine impact. Natural history
growth is excluded. Preserved means configuration preserves the relevant prefix;
it does not promise a provider cache hit. Unproven changes require adoption.

Whether a model or catalog edit reaches an existing Session at all is decided by
the effective invocations an Attempt of that Session would freeze — its primary
and explicit Summary selection and those of its admitted named Agents — resolved
exactly as admission resolves them: provider endpoint and credential source,
wire model, protocol, limits, compat, effective capabilities, the resolved
Profile, reasoning state, output budget and request parameters. An omitted
`profile` and an explicit `profile` naming the default resolve to the same
invocation, and an edited, added or removed Profile that no such selection
resolves through changes nothing for the Session: it prepares and adopts
nothing, exactly as for an unselected Model. The catalog edit is still
published — new Sessions start from it, and a later `session/setModel` resolves
against the current sources — while a Session's own catalog view stays its
adopted generation until it next adopts.

`session/adoptConfiguration` addresses a concrete ready candidate and expected
Session binding revision. It returns typed Busy, NotReady or Conflict, or commits
the complete binding atomically under the same gate as Attempt admission. It never
cancels work, edits history, compacts, invokes a model or changes the selected model.
A ready candidate is not an adopted binding.

An independent Attempt captures complete execution configuration once. Requests,
Steps, retries, recovery, Tool batches, Subagents and Workflow children inherit
that capture even after new policy publishes. Session creation resolves its model
once; changing global defaults does not replace it. `session/setModel` is the
single deliberate Session model mutation and uses the same preparation/commit
primitive, with Session and model baseline fences.

`configuration/reconcile` rescans external files or retries failed preparation.
Stable changed inputs create a new desired revision; retrying unchanged inputs
creates a new application attempt. Healthy semantic no-ops do not advance runtime
generation or rebuild resources. There is no filesystem watcher.

Effective and source projections report simultaneous per-unit results: applied,
preparing, ready for adoption, failed, and process restart required. Actual process
bindings are separate from desired state. Connection/residency admission limits
apply through their owners; the process drain deadline waits for restart. Fixed
startup storage/transport bindings remain process-owned. Notifications identify
scope and monotonically ordered native versions; clients discard stale versions.
Reconnect rereads authority and never replays uncertain mutations.

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

Global Settings authors User definitions; explicit Workspace Settings authors the
registered Workspace without Session focus or runtime allocation. Session Settings
owns only durable Session selections/status. User and Workspace edit native structured drafts for Providers, Models, Root
selection, policies, Plugins, MCP definitions and complete named-Agent profiles.
The User config pathname and fixed User resource root are shown separately.
Save starts native application automatically. Settings presents independent
application, failure/retry and actual-versus-desired process state. The only ordinary
Web adoption surface is below the focused Session title, using the runtime's published adoption eligibility.
CAS conflicts preserve drafts. Rescan is a diagnostics action.

TUI `/settings` authors explicit User/Workspace sources; `/session settings` inspects
Session selections and application, and `/session adopt` submits the inspected
candidate and binding. `/model` changes Session selection. Neither client parses,
merges or classifies configuration. App Server protocol 16 is generated from Rust; obsolete
development protocols are rejected without compatibility decoding.

CFG3 intentionally replaces the previous development configuration and durable
formats. There are no compatibility readers, aliases, trust commands, migration
shims or persisted-effective-config fallbacks. Old split configuration files are
not read. Incompatible durable schemas are refused. Author the current layout
explicitly; retain old development data separately if needed.

## Child authority across activation boundaries

A native Agent's resolved profile/resources and effective execution policies
freeze at Agent creation and remain fixed across `send_message` resume. Reloads
and Session configuration changes do not re-resolve that child's authority.
`subagents.max_concurrent` limits simultaneous finite child activations, not the
number of inactive durable identities. Job controls and Agent controls are
separate fixed foreground control Tools. See [Jobs and Agents](jobs-and-agents.md).

## Image Tool and Bash presentation (#412)

See [the image and Bash contract](image-reading.md) for effective capability
intersection, Attempt-frozen publication, managed image ownership, provider
transport, text-only history projection, and presentation-only Bash descriptions.

An image-capable declaration uses `input_modalities = ["text", "image"]` in
the Model's capabilities table with any supported protocol: `anthropic_messages`,
`openai_responses`, or `openai_chat_completions`.
Include `"read_image"` in `agent.tools.builtin` to express Tool intent.
Image covers both User and ToolResult inputs. Chat Completions assumes the
extended OpenAI-compatible/vLLM multimodal Tool contract; incompatible endpoints
return ordinary provider errors without fallback. Text-only declarations stay
text-only and do not activate the Tool.

For example, replace the endpoint, credential variable, wire model ID and limits
with those of an image-capable deployment:

```toml
[providers.vision]
base_url = "https://api.example.invalid"
api_key = "$VISION_API_KEY"

[models.vision]
provider = "vision"
id = "image-capable-wire-model-id"
protocol = "anthropic_messages"
context_window = 128000
max_output_tokens = 4096

[models.vision.capabilities]
input_modalities = ["text", "image"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[agent.model]
model = "vision"

[agent.tools]
builtin = ["read", "read_image", "bash"]
```

### MCP settings observation

Settings opening, closing, scope switching and refresh only inspect sources.
They never connect, reconnect or disconnect an MCP server. The existing native
capability generation owner remains the only runtime connection owner. Native
prospective preparation and Root selection are separate from admitted runtime
state; absent live connection evidence is explicitly not observed.

Ordinary `configuration/sourceWrite` rejects new literal MCP environment/header
values. Use `sensitive_env` / `sensitive_headers` environment references instead.
Existing literals may be retained by key or removed, on the exact source revision;
replacing them requires editing through the native source owner, outside the Web
configuration protocol. No Host credential-writing API is introduced.

The earlier trust-gate planning in issues #303/#312 predates accepted CFG3:
there is no native Workspace trust gate. Product Host Workspace authorization
remains distinct from native whole-entry User/Workspace precedence.
