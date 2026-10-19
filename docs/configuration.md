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
`local_runtime::authoring::RuntimeLayer`. Unknown fields are rejected by the
parser; the App Server deletes them from a source at its native boundaries
before parsing, together with a few other exactly recognizable constructs (see
[Automatic source repair](#automatic-source-repair-464)). TOML tables and arrays
preserve authored omission versus explicit empty values. The current document
version is `schema_version = 10`.

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
generation, and ordered as authored. Root has no child timeout field; it
belongs to named profiles.

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
| Native selection | Complete `agent.tools.builtin` array | `[]` selects no ordinary native Tool; it never suppresses the Agent control group |
| MCP selection | Complete `agent.tools.sources.<server>` value | `[]` selects none for that source |
| Python selection | Complete `agent.tools.sources."python:<package>"` value | `[]` selects none for that package |
| Skill visibility | Complete `agent.skills` value | `[]` exposes none; `"all"` exposes valid eligible packages |
| Plugins | Complete `agent.plugins.<plugin>` object | Empty object defaults off; omitted contributors use that Plugin's defaults |
| Agent delegation | Complete `agent.agents` array | `[]` permits no named Agents and composes no Agent controls |
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
project guidance and optional `timeout_ms`. There is no `worktree` table:
every Agent runs in the Session's shared workspace, and an authored `worktree`
table is refused (#462). See [`agent.schema.json`](../schemas/agent.schema.json).
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

Every source transaction has a physical authority as well as a byte revision.
When it begins, the writer opens the document's directory from `/` one
component at a time without following any symlink (source paths are canonical:
a Workspace is validated canonical, the User binding is normalized when bound)
and keeps that directory handle and its device/inode identity. The read, the
writer lock, the staged file, the atomic replacement (`renameat` inside the
handle — the linearization point) and the directory sync all go through that
one handle, so replacing or redirecting any component of the pathname cannot
move the write into another directory. Opening the directory — and creating its
missing components, for a new document — claims nothing about durability.
Immediately before the replacement the transaction confirms the directory's
path: it walks the pathname from `/` again without creating or following
anything, syncs the parent of every component — whether this transaction
created it, another writer created it a moment earlier without syncing yet, or
it was merely visible — and requires the walk to end at the admitted directory
(same device and inode); the handle is never replaced by what the walk opened.
The document must then still have exactly the expected bytes. A redirected
target is refused as `invalid_params`, as an invalid target is at entry; changed
bytes are a `source_conflict`; a confirmation sync that fails while the pathname
still names the directory is an I/O failure — all leave every source unchanged.
A committed write therefore means the published entry and every name needed to
reach it after a crash are durable, as this transaction itself confirmed them. The writer lock is
advisory: it orders rustX writers, not an external rename of the directory
itself. A directory renamed between that check and the replacement still
receives the bytes, but only the physical directory the transaction admitted;
the writer checks the pathname again after the replacement and reports
`committed_durability_uncertain` when it no longer names that directory, as it
does when the directory sync fails. An uncertain write is never replayed.

Writer artifacts have fixed-length names, so every basename the filesystem
accepts for a source document stays writable. The persistent writer lock beside a
document is `.rustx-lock-<sha256 of the exact basename bytes>.lock`: every native
writer of that document — source authoring, automatic repair and create-only
initialization — locks the same inode exclusively, and distinct documents never
share one. Writers create it; nothing unlinks it.

Every reader of a `rustx.toml` has the same physical authority as a write.
Session resolution — Session creation and loading, configuration application
capture and preparation, the App Server process policy, `rustx config check`
and `rustx config show` — reads both documents through the same admission as
source capture and repair: admitted once, never followed, a missing directory
an absent document, and reread at the end of the capture through the admitted
directory, which the pathname must still name. A symlinked source, or a
directory replaced or redirected during the capture, is refused as an
unavailable source rather than answered from the replacement, even with
identical bytes. Session resolution holds no writer lock: rustX writers
publish only by atomic replacement after every check, so its revision fence —
every document and resource reread after the capture — makes a concurrent
write an explicit "changed during capture" failure, never a mixed capture, and
never blocks a writer. The User source pathname is bound once, when the
process binds `--config` (or the default) to its canonical path; a symlink
named there is resolved at binding, and a symlink appearing at the bound
pathname afterwards is refused like any other.

Source capture (`configuration/sourcesRead`, the read after a write and
coordinator capture) is read-only: it creates no directory, lock, staged file
or backup and changes no source. Each round admits every document's directory
once, from `/` without following any symlink and without creating anything (a
missing directory is an absent document); the shared writer lock, the bytes and
their reread are all relative to that one handle, each opened
`O_NOFOLLOW | O_NONBLOCK` and checked as a regular file on the opened
descriptor, which is then read within the 1 MiB bound. The pathname is never
resolved again to reach the document, so a symlink, FIFO or directory swapped in
at any moment is refused without its target being opened. Before a round
answers, the pathname must still name every admitted directory — including a
directory that was missing and is admitted again for the reread: a directory
replaced or redirected during the capture is refused as `invalid_params`, as at
entry, never answered from the replacement, whatever else the round reports.
Repair's own reads of a `rustx.toml`
use the same admitted read. It holds each
document's existing writer lock *shared* for the whole capture, opened without
creating or following it, so every rustX writer of that document is excluded and
other readers are not. A document without a lock has never had a rustX writer;
the capture is accepted only if it still has none afterwards. A lock that
appeared meanwhile means a rustX writer may have run unexcluded — even one whose
bytes changed back — so the capture is taken once more, now holding that lock: at
most one more round than documents. Within a round every document and resource
revision is reread after the facts are derived, so a writer outside rustX, which
no advisory lock excludes, makes the capture an explicit revision conflict rather
than a mixed snapshot. A staged candidate is `.rustx-stage-<uuid>.tmp`, created exclusively
and removed unless it is renamed into place. A document's basename is a native
filesystem name, not text: reading, locking, staging, replacement and removal use
its exact bytes, so a name that is not UTF-8 — where the operating system and
filesystem allow one — is written like any other. The App Server process
bindings still require a lossless UTF-8 representation for the JSON protocol.

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
resolves through changes nothing for the Session's invocation: it prepares and
adopts nothing, exactly as for an unselected Model.

Catalog publication is a separate contract from invocation adoption. A valid
source generation's Model Catalog — the Provider unit: `models`, `providers` and
the new-Session default `agent.model` the catalog is validated with — is the
authority for future selection. It is published to an existing Session without
preparation when every invocation of that Session is unchanged, and also when the
Session's own configured Model or Profile is one the new catalog no longer
admits: the catalog is what the replacement is chosen from, and a stale Session
selection never makes a valid catalog unpublishable. The Provider unit settles on
its own: an Instructions, context or Capability change in the same generation
neither holds it back nor is adopted by it. That includes the named Agents Root
admits and their definitions, which are the Capability unit's until it adopts: a
newly admitted Agent is not an invocation of the adopted closure, and an Agent
naming no model inherits the invoking Attempt's. What publication must preserve
is each adopted Agent's own explicit selection, which resolves against the
published catalog. Those units are then prepared as
usual against the binding just published, and their candidate waits for
adoption as before, so the Session can discover and select a replacement first.
An explicit selection committed meanwhile recaptures the generation over the new
selection; the older candidate's expected binding no longer matches and it
cannot adopt. Under the gate Attempt
admission takes, the Session's selection authority, the catalog its configuration
generation carries and its retained binding are committed together, all or
nothing, and the configuration generation advances, while the current resolved
invocations, their provider adapters and the capability snapshot are retained
verbatim. From that commit, `session/models` advertises the published catalog
and `session/setModel` (and Runtime Client `model_catalog`/`model_set`) resolve
against exactly that catalog, never unpublished sources. An admitted Attempt
keeps the snapshot and registry it froze. A failed or overtaken generation never
publishes, so the last good catalog remains the authority; a catalog the source
document cannot validate fails that generation's context units with it. When the
edit changes an invocation the Session can still resolve — its own, or what an
adopted named Agent's explicit selection resolves to — the catalog arrives with
that change's adoption,
together with any capability or context change of the same generation. A cold
Session receives the catalog when it next becomes resident. New Sessions start
from the Workspace's published creation catalog.

A configured selection the published catalog does not admit is **unavailable**.
The Session keeps it configured — nothing switches it to the catalog default or
drops an explicit Summary policy — and `session/model` reports it in
`unavailable` with the native diagnostic. Its last resolved `effective` and
`summary` remain display facts only; a Session loaded with an already unavailable
selection, after residency loss or a restart, has none. While unavailable, the
Session admits no model work: `turn/start`, `turn/steer` and `context/compact`
are refused with `model_unavailable`, and inbound accepted before the publication
is held, never run on the removed selection and never dropped. Held work does not
make the Session busy for the correction. The correction is an ordinary
`session/setModel` with a complete selection the
catalog admits, primary and explicit Summary alike; it commits through the usual
fence and admits held work with exactly the new selection.

The adopted generation's `source_revisions` stay the source manifest its context
was resolved from. Independently published units — execution policy, shared
capacity and the Model Catalog's Provider unit — carry their own component
revision with their values and provenance, so a source file that also holds
unrelated, not yet applied changes never makes those changes look adopted.

`session/adoptConfiguration` addresses a concrete ready candidate and expected
Session binding revision. It returns typed Busy, NotReady or Conflict, or commits
the complete binding atomically under the same gate as Attempt admission. It never
cancels work, edits history, compacts, invokes a model or changes the selected model.
A ready candidate is not an adopted binding.

An independent Attempt captures complete execution configuration once. Requests,
Steps, retries, recovery, Tool batches, Subagents and Workflow children inherit
that capture even after new policy publishes. Session creation resolves its model
once; changing global defaults does not replace it. `session/setModel` is the
single deliberate Session model mutation. A validated complete selection commits
immediately, including while an Attempt, child or background Tool owns execution.
It is durable Session authority for future independent Attempts; it never replaces
an admitted invocation or waits in a pending-model queue.

Selection preparation uses the exact catalog and retained binding published to
that Session, outside the commit fence. The final commit revalidates the captured
source/application, settings, binding, resource and runtime identities. The native
coordinator gate serializes that commit with independent Attempt admission: a
selection committed first is captured by the new Attempt; an Attempt admitted
first retains its complete previous snapshot. Concurrent or superseded candidates
fail instead of overwriting newer authority. Model selection never imports pending
instructions, capabilities or other source changes. Explicit context adoption
retains its separate readiness, cache-impact and idle-safety rules.

The whole `SessionModelConfig` is replaced together. A same-Model Profile choice
preserves request overrides, output override and Summary Model policy; native
validation rejects any newly incompatible combination. Choosing a different Model
resets primary request/output overrides and uses that Model's default Profile
unless a Profile is explicitly chosen, while retaining the independently selected
Summary Model policy. There
is no fallback to another Model or Profile after validation failure.

The selected Session Model/Profile and an active Attempt's Model/Profile are
separate authoritative facts, including when only effective parameters differ.
Every later Turn, Tool continuation, retry and compaction summary of that Attempt
uses its captured invocations, limits and adapter bindings. A child created by the
Attempt after a Session selection change inherits the parent's captured model
authority and resources. Snapshot and capability leases retain physical resources
until their existing owners settle; selection does not retire resources early.

For a changed selection, the atomic catalog replacement that publishes the Session
settings is the commit point. Retained-binding and resident-runtime publication follow
while the admission fence remains held, before any later Attempt can capture.
A failure before that replacement leaves all model authority unchanged. If replacement
committed but its durability acknowledgement is uncertain, native authority stays
coherent with the published settings and the operation reports uncertainty rather
than success. Clients reread native authority after conflict, disconnect or an
uncertain response; they never automatically replay the mutation. Unload/reload
and reconnect preserve the selected Session state and do not rewrite frozen
execution facts.

An already-persisted identical selection is a no-op only after the same lifecycle,
revision and resource checks. It writes nothing and advances no binding or event.
It also preserves Application Identity, Version, Unit states, and any Ready
Candidate, and starts no configuration preparation. Selecting the inherited
current model explicitly is a durable intent change, even if its invocation is
identical. Clients submit the complete selection and let the native owner decide.

Durable local Session runtimes reject runtime-only `model_set` calls, including
Runtime Client host, attachment and request-dispatch controls. Only the native
Session owner may commit their settings, retained binding and runtime authority.
Standalone Conversation runtimes retain their in-memory control operation.
An invocation that happens to resolve alike still visits the durable owner: making
an inherited default explicit must persist that intent. Selection does not create
a source-application scope when the Session has no source application to reconsider.

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

## Automatic source repair (#464)

The App Server repairs a small, exactly recognizable set of invalid authored
constructs in the two `rustx.toml` sources — the bound User document (including
a process `--config` override) and the selected Workspace document — with the
same rules for both. The strict parser is never weakened: repair rewrites the
file, and the rewritten file must then pass the same validation every consumer
applies. Repair is deletion-first and deterministic; it never guesses, renames,
coerces or invents a value.

| Authored problem | Automatic transformation |
| --- | --- |
| Field unknown to its closed schema object — including inside policy, approval, security, plugin or isolation objects | The field and its value are deleted; there is no semantic exemption |
| Unknown configuration table | The table is deleted; unrelated tables stay |
| Unknown entry in `agent.tools.builtin` | That element is deleted |
| Repeated ordinary `agent.tools.builtin` entry | Later occurrences are deleted; the first stays |
| Agent control or Plugin-owned Tool in `agent.tools.builtin` | That element is deleted, with its classified reason |
| Unsupported key under `native_tools` | That policy entry is deleted |
| Explicit `schema_version` other than the current version (older or newer) | Set to the current version, only if the whole candidate then validates |
| Omitted `schema_version` | Stays omitted |
| Invalid TOML syntax, wrong type or value of a known field, missing required facts, broken Provider/Model references, invalid `request_params` JSON, `app_server` in a Workspace, any other remaining failure | Nothing is edited; the source is unchanged and reported `unresolved` |

Eligibility comes from the generated source schema of `RuntimeLayer`
(`schemas/rustx.schema.json`): only keys of closed objects
(`additionalProperties: false`) are fields. Declared open maps — Provider,
Model and Profile identities, `agent.tools.sources`, `environment`,
`mcp_tool_policies` — hold authored identities, never unknown fields, and the
JSON inside `request_params` is an opaque string that is never entered. A union
is entered only through the one alternative that admits the authored shape, so
an unknown `mode` tag selects nothing and nothing beneath it is touched. Tool
entries are classified by the native Tool domain itself (the Agent control
vocabulary, the extension plane and the definitions a root profile can select),
which is also the owner of the strict `agent.tools.builtin` validation.

Candidate construction uses `toml_edit` in one finite pass over one exact byte
revision: comments, formatting, quoting, secrets and every unrelated value stay as
the editing model allows, and a second plan of the candidate must find nothing
left to do. The candidate is valid only if the production source parser accepts
it (types, required fields, `request_params`, Workspace `app_server`, the current
schema version) and it satisfies one native semantic contract that Session
resolution and source inspection share:

- **Source-intrinsic constraints always apply**, with or without a default
  model: App Server process policy, every Provider and Model entry on its own
  terms (identities, endpoint, credential source, limits, capabilities, Profiles,
  compatibility fields, Model `request_params`), and every root runtime
  constraint the document decides alone (context and deadline policies, the Tool
  environment, the root Agent profile and Tool selection, subagent concurrency,
  admission and Workflow identities).
- **Composition-dependent constraints apply once the composition is known**: a
  Model's Provider, a non-empty catalog, the selected models, their Profiles and
  overrides, and the context budgets they bound. A source may leave these facts
  to the other scope. A Workspace is composed over the current User source; the
  User is composed with the selected Workspace at Session creation, load and
  reconcile, and is checked on its own terms only at startup, where no Workspace
  is selected and none is scanned. An unreadable or itself invalid other source
  supplies no facts, so it never blocks this source's repair. A composition
  without a default model is incomplete, not invalid, once everything else holds.

Every candidate is also bounded in bytes: its complete serialization must fit the
1 MiB limit every authored file reader enforces (`bounded_file`), checked before
any lock or backup. A repair that would grow a source past it — `schema_version
= 9` becoming `10` in a document already at the limit — is `unresolved` with no
backup, and the source is unchanged. The source publication primitive fences the
same bound for every write, so `configuration/sourceWrite` refuses an oversized
document as invalid; nothing is published that its own reader would refuse.

`.agents` resources (MCP servers named by `mcp_tool_policies`, Agents, Skills)
are not source facts. Nothing here resolves credentials, connects MCP, prepares
Python, creates a Session or calls a model.

A changed source is committed through the same native writer primitive as
`configuration/sourceWrite`, under the same physical authority: the directory
observed when the observation read the source, the source writer lock, an exact
revision and identity check before the candidate is used, a staged and synced
file, the confirmation of the source directory's path and the exact revision
check immediately before publication, atomic replacement and a
parent-directory sync. Before that, an
exact copy of the original bytes is written to private storage under the bound
runtime root (`<runtime_root>/configuration-backups/`, directory `0700`, file
`0600`, never inside the Workspace), named by source identity and original
revision; a backup failure leaves the source unchanged. The backup is confirmed
only by the transaction that publishes the source, and only once every name
leading to it is durable: a missing component of the backup path — runtime-root
ancestor, runtime root or backup directory — is created, and the path is then
confirmed exactly as a source directory's is, before anything is copied.
Visibility is not durability, so nothing relies on another writer's sync or
cleanup: a failed sync fails the backup and removes nothing, and the next
transaction confirms a leftover entry before using it. The copy is then linked
and the backup directory synced; only then may the source be replaced, and only
then does a receipt report a backup.

`repaired` therefore guarantees that the exact original bytes are durably
backed up outside the Workspace, that the candidate replaced the source
atomically, and that the replacement and every name needed to reach the source
— each confirmed by this transaction, whoever created it — are durable. A
failure to confirm the source directory's path before the replacement is
`unresolved` with the backup reported and the source unchanged; nothing is
retried.

Observation is separate from durable writes and creates and syncs nothing:
reading a source, finding it healthy and admitting a Session's scope cost no
durability I/O, and a healthy source gets no lock file, staged file or backup. The bound runtime root is
the trust anchor (canonical when bound). Each backup opens it the way a source
directory is opened: walked from `/` one component at a time without following
any symlink, missing components created relative to their verified parent, and,
whenever the boundary has selected a Workspace — for the User source as much as
for the Workspace source — checked to be physically disjoint from that admitted
Workspace (following `..` from both handles). The backup directory is then created or opened relative to that handle and verified
as the current user's `0700` directory; the staged copy, its no-clobber
publication link, the comparison with an existing copy and the directory sync all
go through that one handle, so a symlink in any ancestor, or replacing any
component after admission, cannot redirect the bytes. Physical safety does not
depend on the pathname afterwards; confirmation does: the backup counts only if,
once durable, the bound path still names the directory holding it. Otherwise the
copy stays in the admitted (moved) directory, the receipt is `unresolved` without
a backup and the source is unchanged. A healthy source is read
once without a lock or any write, so it gets no lock file, no backup and no
receipt. An external edit that races the repair, or a Workspace pathname that no
longer names the admitted directory, supersedes it: nothing is published, nothing
is reported and the next boundary plans from whatever the target names then. A
failure before replacement leaves the source unchanged; a replacement that may
have happened, a parent sync that failed, or a pathname that stopped naming the
written directory after the replacement is reported `commit_uncertain` with the
revision observed by rereading the admitted document (unknown once the pathname
names another directory), and is never replayed. User and Workspace are independent
transactions, so one may be repaired while the other stays unresolved.

Repair runs only at native source boundaries:

- App Server startup repairs the bound User source after the runtime root is
  opened and before process `app_server` policy is parsed or any transport opens;
- Session creation, and a Session load that has to resolve configuration afresh,
  repair the User source and the exact selected Workspace document first;
- `configuration/reconcile` repairs the User source and, for a Workspace target,
  that Workspace before it captures them.

A boundary that has selected a Workspace admits it before either source is
touched. The Workspace must be a canonical directory disjoint from the bound
runtime root under the rule Session resolution applies (neither contains the
other, checked on the canonical paths), and then physically: both are observed
as no-follow directory handles and neither may lie within the other. Admission
creates nothing; runtime storage that does not exist yet is checked against the
admitted Workspace by the backup that creates it. A Session locates its Workspace exactly as its resolution
does, so a request whose Workspace does not resolve selects none. A scope that
fails admission, or a request that selects no Workspace, repairs neither source —
no backup, no rewrite, no receipt, nothing for the coordinator — and the Session
resolution that follows rejects it as before. The admitted Workspace handle is
the one every transaction of the boundary uses: the Workspace source is
published through it, and both sources' backups are checked against it, so a
Workspace pathname replaced after admission can neither receive the Workspace
repair nor host a backup. Startup has no selected Workspace and proves nothing
about Workspaces chosen later; it neither scans nor invents one, and later
boundaries admit their own. Once a scope is admitted, User and Workspace remain
independent transactions: a later failure of one never rolls back the other.

`rustx config check`, `rustx config show --sources`, `configuration/sourcesRead`
and every client navigation stay read-only. There is no watcher and no periodic
check. A committed repair is handed to the configuration coordinator exactly like a
committed `configuration/sourceWrite`, under the same lock, so one commit is
captured once and application, preparation and Session adoption keep their
existing contracts: a repaired file is desired state, not proof that anything was
applied. A receipt never enters canonical history, the Event Journal or model
context.

Each source keeps its latest repair observation in a finite, process-local
ledger: `repaired`, `unresolved` or `commit_uncertain`, the exact before and
committed revisions, the change count and up to 64 changes (the schema object an
unknown field was removed from, a removed Tool selection's position and reason
with its name only when that is a native Tool, the schema version update),
whether more were
omitted, whether a backup exists, and for a failure a diagnostic built only from
native constants: the parser's or the semantic owner's fixed category, line and
column, and a schema location in which every Provider, Model, Profile or other
open-map identity is `*` (a path the schema cannot explain is dropped). It never
holds a removed value, an invalid value or identity, a domain error message, a
source excerpt, a credential, a `request_params` payload, an authored key or
identity, an unsupported Tool name or backup content: repair edits by exact
authored keys, but a receipt reports only their projection onto the source schema
(property names, `*` for an open-map identity, `[n]` for a position).

A receipt's `id` names one repair occurrence, separate from its content and from
the ledger order. Every publication that happened or may have happened is a new
occurrence, so restoring the original bytes externally and repairing them again
reports a second receipt with a new id, even though its changes and revisions
equal the first. Rereading, redelivering or re-observing a receipt keeps its id.
An unresolved observation equal to the source's current unresolved receipt is
that episode again and changes nothing; once the episode was withdrawn, the same
invalid bytes start a new one. A superseded transaction publishes nothing and
reports nothing.

Every change the ledger publishes — a new receipt, a replaced one, a withdrawal
— advances one process-wide sequence, under the ledger lock that applies the
change; the least recently observed Workspace is forgotten beyond the bound.
That sequence is the only order of observations: a notification carries the
sequence of its change, and a source read takes the receipts and the current
sequence together under the same lock, so the read's sequence is never lower
than any change it reflects — including for a Workspace the ledger has
forgotten, which reads as having no receipt. Receipt ids name occurrences; they
never order them. A receipt is withdrawn only when its source is found intrinsically healthy;
a client deciding what to present reconciles against that order, never against
which receipts it already showed (see [TUI and App Server](tui-app-server.md)
and the [protocol](app-server-protocol.md#automatic-source-repair-receipts-v51)).
A connection's interest is the User and the 64 Workspaces it most recently
addressed. Each admission — including addressing a forgotten Workspace again —
delivers the source's current level once, at the sequence it was read at: its
receipt, or none when the source reports no repair, whether withdrawn, never
recorded, or forgotten by the ledger. A Workspace the connection keeps
observing whose delivered receipt the ledger then forgets is delivered that
absence once, without being addressed again. That empty level is a state, never
a repair, and presents nothing; it reports what the ledger retains, never that
the document is valid, which only the source read's diagnostics establish. The TUI and the Web console retain the same
finite number of observations — the User and 64 Workspaces, the least recently
observed Workspace forgotten first — replace an observation only with one of a
higher sequence (even of the same receipt, which is not presented again), never
adopt a conflicting one at an equal sequence, recover a forgotten source by a
read-only source read rather than assuming it, and word a combined notice by
its most severe receipt: `commit_uncertain`, then `unresolved`, then
`repaired`.

A source is healthy under one source-intrinsic contract: the production strict
parser and source restrictions accept it, every source-intrinsic constraint
above holds, and no repair is eligible. Nothing being eligible proves nothing on
its own: a parsable document violating a source-intrinsic constraint (for
example `subagents.max_concurrent = 0`) is left unchanged and reported
`unresolved` with that constraint's location. Facts another scope may supply —
Model definitions, the default selection — are never part of it. An unresolved
receipt is withdrawn only when the source satisfies this contract, at a later
boundary or after a committed `configuration/sourceWrite`; a committed write that
leaves it still invalid replaces the receipt with the current observation. A
write never creates a receipt for a source that had none.

A semantic failure of Session resolution names the document that authored the
failing field, decided from the overlay provenance of the composition: every
authored unit containing the field, or contained in it, must come from the same
User or Workspace document (process `app_server` policy is User-only). A failure
whose field spans both documents — such as a Model naming an undefined Provider
when both documents define Models — names no file: it belongs to the
composition.

Manual `repair_config` remains the editor for documents that fail their own source
contract — syntax, types, Workspace `app_server`, a foreign schema version —
including TOML-parsable ones; ordinary semantic-unit writes require a document that
satisfies it, and the source projection marks a document violating it as not
loaded.

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

### Automatic Agent controls (#462)

`subagent`, `list_agents`, `send_message`, `wait_agent` and `interrupt_agent` are
one runtime-composed capability group of the Root Agent. They are present
together, exactly once each, when `agent.agents` admits at least one valid named
Agent, and absent otherwise. Discovered but unselected definitions, Workflow-only
Agent dependencies and existing durable Agent identities do not compose them.
None of the five names may appear in `tools.builtin` of the Root or a named
Agent, in an invocation override, or in a native policy table: such an entry is
rejected at the configuration boundary instead of being ignored, and an empty
`tools.builtin` does not suppress the group. Named and Workflow child runtimes
never receive it. A Root that admits Agents without the subagent registry fails
composition rather than publishing a zero-tool state. Effective inspection
reports the group as `agent_controls`, separate from the ordinary selection.
Removing every admission removes the model-facing tools for new Attempts only;
existing durable Agents keep their authorized typed App Server controls.

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

The MCP settings list is passive: opening, switching scope, searching and Refresh
only inspect configuration. An explicit per-definition Test connection action
starts a finite native `mcp/probe` diagnostic rather than a Session connection.
Each check captures one exact
source document revision, performs initialization and `tools/list`, then awaits
transport/process cleanup before returning. No business tool is called. The
indicator describes the last configuration connectivity check, not live Agent
availability. Missing transport evidence stays unknown.

User and Workspace rows use the same diagnostic contract. Each check reads only
the selected source, including a shadowed User definition; it does not resolve a
same-name definition from another scope. Workspace checks pass through the
Product Host registration lane. Source reads remain inert. Runtime capability
ownership and running Session resources are unchanged.

Ordinary `configuration/sourceWrite` rejects new literal MCP environment/header
values. Use `sensitive_env` / `sensitive_headers` environment references instead.
Existing literals may be retained by key or removed, on the exact source revision;
replacing them requires editing through the native source owner, outside the Web
configuration protocol. No Host credential-writing API is introduced.

The earlier trust-gate planning in issues #303/#312 predates accepted CFG3:
there is no native Workspace trust gate. Product Host Workspace authorization
remains distinct from native whole-entry User/Workspace precedence.
