# Root and named Agent profiles

Configuration ownership, complete syntax and overlay rules are specified in
[CFG3 configuration](configuration.md). Root selection lives in `rustx.toml`.
Named profiles live in the two canonical `.agents/agents/<name>.toml` roots.
Workspace replaces a same-name User profile completely, including when invalid.

```toml
# rustx.toml: the Root profile
[agent]
instructions = "Coordinate review."
skills = []
agents = ["reviewer"]
workflows = []
[agent.model]
model = "review-model"
[agent.tools]
builtin = ["read"]
```

```toml
# .agents/agents/reviewer.toml: a complete independent child profile
# Omitted model inherits the invoking Attempt's frozen effective model.
description = "Review changes"
instructions = "Inspect the diff and report actionable findings."
skills = ["review-guide"]
timeout_ms = 120000
[tools]
builtin = ["read", "grep"]
[tools.sources]
github = ["get_diff"]
"python:analysis" = "all"
[plugins.todo]
enabled = true
[agents_md]
inherit = true
[worktree]
enabled = true
require_clean_parent = true
```

| Dimension | Owner and meaning |
| --- | --- |
| `model` | Root default or independent named selection. Omitted named selection inherits the invoking Attempt's complete frozen model. |
| `tools.builtin` | Exact native whitelist; omission selects none. |
| `tools.sources` | MCP identity or `python:<package>` to `"all"`, exact Tool names, or `[]`. Definition alone grants nothing. |
| `skills` | `"all"`, exact names, or `[]`, for both Root and named Agents; default none. Prompt visibility only. |
| `plugins` | Closed Rust-owned Agent Status, Todo and Goal vocabulary, default off. Each Root Plugin object overlays atomically. |
| `agents`, `workflows` | Explicit delegation/invocation allowlists. Existing one-shot child scope limits remain enforced. |
| `description`, `instructions` | Agent prose; named reusable profiles require nonempty values. Prose cannot change invocation policy. |
| `agents_md` | Canonical project guidance and bounded supplemental files. |
| `timeout_ms`, `worktree` | Named child lifecycle/workspace policy only. Root rejects these fields. |

Root may delegate to reviewer while lacking reviewer's Grep or external Tools.
There is no Root Tool or Plugin ceiling over the named profile. Invocation
parameters remain bounded execution data under the existing override admission
owner; they are never another persistent configuration layer.

Skill discovery captures both absolute roots and shadows whole packages before
parsing. A malformed Workspace winner never reveals the lower User package.
Unused invalid resources produce ordered bounded diagnostics. Selecting an
invalid or unavailable resource fails typed resolution/admission. Root and named
Skills share progressive disclosure: the prompt identifies the two absolute
roots and selected names/descriptions without enumerating absolute package paths.
Skill visibility does not restrict a file-reading Tool's filesystem authority.

Plugins are explicitly authored closed capabilities. Empty or omitted objects
use their own product defaults, including `enabled = false`. No automatic Todo,
Goal or Agent Status layer grants capabilities. Conversation Todo/Goal state is
owned separately. Child scope restrictions on capabilities requiring ongoing
Root coordination remain typed admission rules, not Root configuration ceilings.

Global Tool execution, concurrency, approval, deadlines and child capacity belong
to the runtime generation. Agent profiles select availability; they cannot alter
those policies. Catalog discovery does not connect MCP or prepare Python. Root
composition prepares Root demand; child preparation materializes only the
admitted child's finite demand through the existing source lifecycle owner.

An Attempt pins one immutable generation. Child resolution consumes that Attempt's
resource/catalog/model snapshot, then freezes exact Tool definitions, source
bindings, Skill bytes and versions, model decision and policies. It does not read
current files or current Root defaults. Subsequent Save or Reload cannot mutate
that child. Cold composition rereads current sources and validates deliberate
Session model intent; materialized Agent profiles are not Session persistence.
