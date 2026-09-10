# Canonical named Subagent resources

## Session ownership and local lifecycle exclusion (Issue #254)

Session deletion cascades along durable ownership, never provenance. `/tree`
nodes belong to the same Session; `/fork` and `/clone` materialize independent
Sessions. Catalog membership and native typed child ownership commits establish
the finite target. Retained worktrees and branches are blockers requiring
explicit disposal, not implicit cleanup targets. Shared environments, capability
resources, caches, config, credentials and project files remain outside it.

Canonical `ProductRoot` identity, `ProductController` admission and target
Conversation lifecycle access are separate. Preflight freezes ownership
transitions, derives native ownership, then locks only target Conversation
allocations exclusively in sorted identity order. A live unrelated Session and
its Runtime Client remain usable; actual target runtime/child/inspection/private
writer access blocks exclusivity. Ordinary activity does not hold the ownership
freeze. Guards release through drop or OS process death; aliases share identity.
The semantic revision hashes only target membership, owned allocations and
final workspace-blocker state, never raw catalog bytes or execution history.
Management reads never create missing stores or directories. See
[the ownership and storage contract](session-deletion-ownership.md) for the exact
lock order, acquisition/release points, participant lifetimes and regression map.


Runtime schema 8 registers role identities in JSONC. Each role's primary
authoring resource is one Markdown file; Rust converts it into the existing
native `SubagentDefinition` and `SubagentCatalog`.

```jsonc
"subagents": {
  "maxConcurrent": 4,
  "definitions": ["reviewer"],
  "main": [],
  "workflow": ["reviewer"]
}
```

The identity `reviewer` resolves to `.agents/subagents/reviewer.md` under the
admitted workspace, or `subagents/reviewer.md` under the known user configuration
directory. Identity is the registered filename stem: 1–64 ASCII bytes, beginning
with a lowercase letter, followed by lowercase letters, digits, `-`, or `_`.
Role files cannot be symlinks that redirect this identity. There is no `id` or `name` frontmatter field, directory-role form, arbitrary
primary path, or alternate extension. Registration arrays reject duplicates,
including duplicates in an overridden lower configuration layer.

```yaml
---
description: Review one bounded request and return a concise result.
model: example/demo-model
timeoutMs: 3600000
tools:
  builtin: [read]
skills: [review-guidance]
agentsMd:
  inherit: false
  files: [.agents/subagents/reviewer/AGENTS.md]
worktree:
  enabled: true
  requireCleanParent: true
---
You are the review subagent. Return evidence for the requested review.
```

## Frontmatter contract

The entire resource must be a regular UTF-8 file of at most 1 MiB. It starts at
byte zero with an exact `---` delimiter line and closes its frontmatter with
another exact `---` line. LF and CRLF are accepted. The body after the closing
line is retained verbatim as primary instructions, subject to the native 64 KiB
instruction bound. A BOM before the opening delimiter is rejected.

The frontmatter is one plain mapping, with at most 32 nesting levels. Unknown
fields, duplicate keys at any depth, non-string mapping keys, malformed YAML,
invalid types, tags (including standard tags), anchors, aliases, directives,
extra documents and YAML merges are rejected. There are no includes, expressions,
macros, inheritance, or generic metadata. Quoted punctuation in ordinary strings
does not enable these features.

The authoritative Rust authoring type is `SubagentDocument`; its generated editor
schema is [subagent.schema.json](../schemas/subagent.schema.json).

| Field | Type and meaning |
| --- | --- |
| `description` | Required nonempty string, at most 512 bytes; routing text only. |
| `model` | Optional `provider/model` reference. Omission inherits the invoking attempt's frozen effective model, including reasoning and request contracts. Explicit references use the native model catalog. |
| `timeoutMs` | Optional integer, 1–86,400,000; the whole-child lifecycle deadline. |
| `tools.builtin` | Exact array of native Tool names; default empty. |
| `tools.mcp` | Map of source identities to exact Tool-name arrays; default empty. Managed Python uses the existing `python:<package>` source identity. |
| `skills` | Exact Skill-name array; default empty. |
| `agentsMd.inherit` | Boolean, default true; include the parent's frozen project guidance. |
| `agentsMd.files` | Ordered supplemental guidance paths; default empty, at most eight. They are distinct project instructions, never the primary role body. |
| `worktree.enabled` | Boolean, default false. |
| `worktree.requireCleanParent` | Boolean, default true; applies when Git isolation is enabled. |

No role field overrides host-owned approval policy, external-source enablement,
credentials, tool execution policies, model capabilities, or Workflow ownership.
Names such as “reviewer,” prose such as “read-only,” and source provenance grant
no authority.

## Roots, replacement, and admission

There are two pinned role slots per registered identity: known user configuration
directory `subagents/<name>.md`, then trusted workspace
`.agents/subagents/<name>.md`. A project resource replaces the entire user
resource: body, tools, Skills and every policy together. There is no recursive
merge, permission union, or body concatenation. With one filename per identity
per layer, same-layer collisions cannot arise from directory enumeration;
duplicate logical registrations are errors. Directory contents are not scanned:
unregistered files, even malformed ones, remain inert.

Configuration arrays replace whole arrays across layers. Resource replacement
does not register a role. Registration does not admit it to either execution
domain. `main` and `workflow` independently select subsets of the registered
identities. Neither admission bypasses source activation, capability validation,
or final model-facing Tool selection.

Project role and supplemental paths must remain inside the admitted canonical
workspace. Project supplemental relative paths resolve from that workspace,
regardless of where `--config` points. User supplemental paths resolve from the
known user `subagents` root and remain inside it. Symlink targets and traversal
are checked against the owning boundary. Missing resources fail the candidate.
These checks do not claim syscall isolation against an actively hostile OS user.
An untrusted project activates no project roles, Skills, Workflows or instructions.

Local launch's automatic Skill roots are the known user configuration directory's
`skills` and the workspace's `.agents/skills`. The standalone Skill discovery
default uses user and workspace `.agents/skills`; neither reads `.rustx/skills`.
Explicit Skill paths retain the existing user/project/CLI ownership validation.
No user files are deleted or migrated.

Implicit project guidance reads at most one file at the admitted workspace root,
using existing precedence: `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`,
`CLAUDE.md`, `CLAUDE.MD`. It never traverses above that boundary or descends into
child worktrees. Global ancestor guidance is not a source. Supplemental role
guidance is explicit and ordered after inherited guidance.

## Checking, reload, and child ownership

`rustx config check` uses the same role loader as runtime preparation. It checks
file bounds, trust, frontmatter, registration/admission references and statically
known model, Skill and Tool/source contracts. It performs no model, Tool, Python,
MCP, package-preparation or network work, creates no Session/runtime state, and
writes no trust. Unknown online Tool identities remain deferred to their source
owner rather than being invented by static analysis.

`rustx config show --sources` reports prospective `roles` keyed by identity:
`identity`, `selected` path, `layer` (`user` or `project`), and optional
`overridden` lower-precedence path. It does not expose role bodies or credentials
and does not claim to describe an existing running Session. Untrusted sources
remain excluded and the normal trust diagnostic explains the exclusion. Invalid
role resources report their source file and registration field path.

Parsing finishes before native catalog construction returns. Registration and
independent admissions validate before capability/model/Skill validation of the
same off-side candidate. `LocalRuntimeResourceLoader::prepare` builds that complete
candidate. `ConversationRuntime::reload_resources` commits capabilities and swaps
`state.resources` under the existing coordinator lock, then emits one coherent
resource observation. Failed or cancelled preparation leaves the previous snapshot
authoritative. There is no additional registry, executor, epoch, or publisher.

`SubagentResolver::resolve` freezes `ResolvedSubagentSpec` from the invoking
generation during native preflight, before process staging and durable ownership
commit. It contains instructions, complete model authority, exact Tool policy,
Skill identities, project guidance, workspace policy, and deadline. A specification
frozen from R1 retains R1 after R2 publishes; a later resolution receives R2.
Normal reload still obeys the existing runtime quiescence requirements.

Child composition and `FrozenSubagentResourceLoader` consume this frozen native
specification. Workspace/Git worktree acquisition supplies physical workspace
ownership only. It never restarts launch resolution, reads role roots, walks an
AGENTS.md chain, discovers Skills, or widens MCP/Python/Tool authority. Skill bodies
retain their established progressive-disclosure semantics.

Schema 8 removes inline role payloads and `instructionsFile`; older runtime
document versions are rejected. There is one authoring path and no compatibility
reader or automatic migration.
