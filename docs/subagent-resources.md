# Canonical named Subagent resources

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
extensions:
  agentStatus:
    enabled: true
    time:
      enabled: false
    background:
      enabled: true
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
| `extensions.agentStatus` | This role's own closed [Native Agent Extension](launch-configuration.md#native-agent-extensions) composition: `enabled` (default true), `time.enabled`/`time.timezone`, `background.enabled`. Omission means this role's built-in defaults, never the invoking root Agent's configuration. |

Role extension composition is **independently authored**: root Agent extensions
and named-Subagent extensions are separate compositions, and a child never
implicitly inherits the root's set. A role that omits `extensions` composes the
built-in defaults, not whatever the invoking runtime happens to run with.

The invoking Agent's own frozen composition does reach the resolver, but only
as **delegation authority** for an explicit invocation override — never as an
inheritance source. A child composes an extension for exactly two reasons: its
definition authored it, or an entitled caller asked for it. See
[Delegation authority](#delegation-authority).

A named role is the **default** child execution profile, not the final one. One
invocation may replace `tools`, `skills`, and `extensions` for exactly that
child; see [Invocation-scoped overrides](#invocation-scoped-overrides). Every
other field — model, instructions, `timeoutMs`, `agentsMd`, `worktree` — belongs
to the definition alone and has no per-call form.

No role field overrides host-owned approval policy, external-source enablement,
credentials, tool execution policies, model capabilities, or Workflow ownership.
Names such as “reviewer,” prose such as “read-only,” and source provenance grant
no authority.

## Invocation-scoped overrides

One `subagent` call, or one Workflow `Agent` node, may carry an optional
`override` object. It has exactly three dimensions and no others:

```json
{
  "agent": "reviewer",
  "task": "Review the implementation",
  "override": {
    "tools": {"builtin": ["read", "grep", "bash"]},
    "skills": ["rust-review"],
    "extensions": {"agentStatus": {"enabled": true}}
  }
}
```

### Replacement semantics

One rule covers all three dimensions:

> A **missing** dimension uses the definition's value. A **present** dimension
> **replaces** that dimension completely.

| Written | Effective result |
| --- | --- |
| no `override` | the definition's `tools`, `skills`, and `extensions`, exactly |
| `"override": {}` | identical to omitting it |
| `"tools": {"builtin": ["bash"]}` | exactly Bash — **not** the role's tools plus Bash |
| `"tools": {}` | no ordinary selected tools |
| `"skills": []` | no selected Skills |
| `"extensions": {}` | no composed native extension |

Replacement is per dimension and independent: an override naming only `skills`
leaves tools and extensions exactly as the role authored them. There is no
additive or subtractive mode, no `addTools`/`removeTools`, no wildcard, and no
recursive merge; a present `tools` or `extensions` object is never merged with
the role's corresponding object.

`"extensions": {}` deserves its own line because the *authoring* document
defaults differ deliberately. A role that omits `extensions` composes the
built-in defaults, which include Agent Status. An override that writes
`"extensions": {}` composes **nothing**: presence, not emptiness, is what
selects an extension in an override, so every closed extension member is
explicitly named or absent. Absent means "not composed", never "use a default".

An explicit `null` is not an accepted spelling for any dimension, or for
`override` itself. Unknown fields, non-goal dimensions, and unknown extension
names are rejected by the same strict boundary that rejects them in role
frontmatter. Duplicate and out-of-order selectors are canonically normalized
exactly as a definition normalizes them, so an override restating the defaults
is the same effective profile as omitting the override.

### Delegation authority

An override says what the caller *asked for*. Whether the caller may ask is a
separate, typed, native decision, and the two launch sites differ:

```text
main `subagent` Tool   dynamic delegation
                       allowed[d] = authorized role baseline[d]
                                    UNION
                                    invoking Agent's frozen admitted profile[d]

Workflow Agent node    trusted static program data
                       allowed[d] = whatever the Workflow's admitted
                                    runtime generation authorizes
```

The union is an authorization **ceiling**, not a merge: it decides what may be
requested, never what the child ends up selecting.

For the main model:

- a role default stays delegable even when the invoking model does not expose
  that capability at all — a named role is an independent projection of the
  generation, not a subset of the parent's toolbar;
- the parent's contribution is its **frozen admitted execution profile**: the
  exact model-facing tool registry of the invoking attempt, the Skills that
  attempt can actually see, and the extension composition its runtime is
  executing against. It is not the generation's whole available catalog, not a
  live mutable registry, not the next generation, and not current configuration;
- a capability known only to the generation, held only by some other role, or
  merely compiled into the executable is not delegable;
- authority is compared by exact native identity — `ToolId`, and
  `SkillId` + `SkillVersionId` — never by display name or role prose. A Builtin
  `search` can never stand in for an MCP server's `search`, and a rewritten Skill
  package is a different identity from the one a caller froze;
- tools, Skills, and extensions are separate authorization domains. Holding one
  never implies holding another, and a dimension the caller did not override is
  never judged against the parent's registry at all;
- extensions are authorized *by configuration*, not by name, and the union is
  taken at that granularity. A composition is not a set of identities, so
  "the role authorizes the whole composition **or** the invoking Agent does"
  would be strictly narrower than a union: it refuses a request whose
  contributors are each legitimately held, only because no single source holds
  all of them at once. Each behavior-affecting contributor is authorized
  independently instead:

  ```text
  Agent Status composed at all  some source composes Agent Status
                                (composing it composes the always-on Todo
                                 contributor, so it is never a free wrapper)
  time.enabled = true           some source composes Agent Status with Time
                                enabled AND the same EFFECTIVE timezone
  background.enabled = true     some source composes Agent Status with
                                Background enabled
  a contributor set to false    narrowing; needs no authority at all
  extensions omitted entirely   narrowing; needs no authority at all
  ```

  So a role holding UTC Time with Background off, and an invoking Agent holding
  Background with Time off, together authorize a child with both on — Time from
  the role, Background from the invoking Agent, nothing manufactured.

  Timezone authority is decided on the zone the child would **render**, never
  on whether `time.timezone` was written. An omitted zone renders UTC, so it is
  a request for UTC and needs an authority that itself renders UTC; a role
  rendering `Asia/Shanghai` does not cover it. Treating absence as
  "unspecified" would have made it a wildcard that manufactured UTC out of any
  timezone authority at all.

A Workflow Agent node's override is compiled program data. It is validated at
compilation and again during resource-generation preparation, and it is not
reachable from model output, node input values, task text, or any expression or
interpolation language. It may therefore legitimately exceed both the role's
defaults and the invoking main model's capabilities — never the admitted
generation.

Authorization is not source availability. Unknown, unavailable, inert, and
unauthorized stay four distinct outcomes, and a requested capability is never
silently dropped.

### What an override never reaches

Model, instructions/body, timeout, workspace/worktree policy, `AGENTS.md`
policy, approval policy, credentials, source enablement, and arbitrary external
configuration have no per-call form. There are no continuable children, no
generic inheritance, no additive or removal syntax, no wildcards, and no plugin
SDK.

Selecting a Skill grants no Tool: Skill visibility and dependency rules are
unchanged, so a Skill selected under an empty tool set stays a Skill selection
and never implicitly adds Read. Conversely, an extension-provided model Tool is
governed by effective extension composition and is never removed by the ordinary
`tools` allowlist.

Extension authorization and child-scope support are independent checks. An
extension a caller is fully entitled to compose still fails deterministically,
before the child is staged, when one-shot child execution cannot own it.

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
file bounds, trust, frontmatter, registration/admission references, statically
known model, Skill and Tool/source contracts, and every statically knowable
reference in a Workflow Agent node's invocation override. It performs no model, Tool, Python,
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
Skill identities, project guidance, workspace policy, deadline, and the
effective native Agent Extension composition. `ResolvedSubagentSpec`, not the
role document and not the raw override payload, is the complete immutable child
execution contract. A specification frozen from R1 retains R1 after R2
publishes; a later resolution receives R2.
Normal reload still obeys the existing runtime quiescence requirements.

The resolution order is the contract:

```text
1. admission      is this role callable in this domain at all
2. structural     is the override a well-formed child selection
3. replacement    effective[d] = override[d] if present else definition[d]
4. dependency     do the EFFECTIVE selections resolve in this generation
5. authorization  may this caller delegate the effective selections
6. child scope    can a one-shot child own the effective extensions
7. freeze         model, instructions, guidance, materialization, identity
```

Step 3 preceding step 4 is what makes a replaced-away default stop being a
requirement: a role whose default MCP tool is offline still starts when the
invocation replaced that dimension, because the offline selector is no longer
part of the effective invocation. The role's separate catalog-admission
validation is unaffected and still rejects a statically invalid definition.

Every failure is decided before a child process is staged, before any
override-specific execution resource is acquired, and long before durable
ownership commits. Cleanup after a spawn is not an authorization boundary.
Cancellation semantics are unchanged at resolution, preparation, and commit: a
pre-commit cancellation publishes no owned child and leaves no partially
materialized resources.

The invoking root Agent's own `extensions` composition participates only as one
half of the explicit delegation ceiling. It is never implicit child
inheritance: a child composes an extension because a definition authored it or
because a caller asked for it and was entitled to.

### Effective execution-profile identity

The two digests are **separate identities** and neither is derived from the
other:

```text
SubagentDefinitionDigest              identity of the SOURCE named definition
                                      (its routing description, its DEFAULT
                                      tool/Skill/extension selections, its
                                      authored spellings)

ResolvedSubagentSpec::profile_digest  identity of the FINAL FROZEN effective
                                      child execution contract, after
                                      replacement
```

`SubagentDefinitionDigest` continues to identify the source definition, and no
longer uniquely identifies one child once overrides exist. The profile digest
obeys one rule:

> The digest identifies the semantic final frozen execution profile, not its
> authoring history, and no behavior-affecting frozen field may be omitted.

So the source definition digest is deliberately **not** part of the profile
preimage. A routing description never executes; a default Tool, Skill, or
extension selection that an invocation replaced completely stops existing
before the child is frozen. Both would otherwise split one effective profile
into two identities. Every behavior-affecting field the definition digest
summarizes is framed directly instead, as its final frozen value.

The identity is derived from the frozen contract rather than stored beside it,
so it is frozen exactly as strongly as the contract and cannot disagree with
the specification it labels; the child recomputes the same value from the same
frozen bytes. The invoking attempt also commits it durably with child
ownership, so it survives a restart unchanged (see
[Durable execution identity](#durable-execution-identity)).

The versioned canonical framing (`rustx-subagent-profile-v2`) covers:

```text
agent name, instructions, workspace policy, execution deadline
frozen model decision   model reference, protocol, context window,
                        model and effective output budgets, reasoning profile
                        and semantics, effective and declared capabilities,
                        compat, effective request parameters
frozen summary policy   "follows the session primary", or an explicit
                        invocation framed by the SAME helper as the primary,
                        so a summary model is identified exactly as
                        completely as the primary one
project instruction chain   path AND content, in order
effective tools         origin, exact ToolId, model-facing name, and the
                        cross-process MCP identity where one exists
effective Skills        exact SkillId + SkillVersionId and the visible name
materialization plane   exactly the external source identities required. The
                        bindings behind them are physical (transport, resource
                        root) or secret (credentials); the one behavior they
                        carry — the invocation policy a server imposes on its
                        tools — is already framed exactly, through each MCP
                        tool's cross-process identity above
effective extensions    the closed composition's EFFECTIVE framing, so an
                        omitted timezone frames as the UTC it renders
```

Values whose serialization carries authoring shape are framed by their
effective semantics rather than through that serializer: an omitted
`time.timezone` frames as the UTC it renders, and `ModelCompat` frames its five
translation decisions rather than only the ones a catalog spelled out. Two
values that behave identically are one effective profile.

The framing deliberately excludes:

```text
source-definition-only  the definition digest, and with it the role's routing
  provenance            description and its replaced-away defaults
desired model config    FrozenModelSpec::configured records what was ASKED
                        for; the resolved invocations are the authority
provider binding        provider identity, endpoints, credential sources, and
                        any admitted credential value
execution identities    SubagentId, conversation/agent ids, tool call id
timestamps              nothing time-derived enters the preimage
physical paths          staging roots, Skill source roots, remapped locations
raw payload formatting  the override's key order, whitespace, duplicates, or
                        whether an override was written at all
```

The provider-binding exclusion is a contract, not an omission: rotating a
credential or repointing an endpoint at the same model leaves the identity
unchanged, on the primary invocation and on an explicit summary alike.

### Durable execution identity

> Once `SubagentOwnershipCommitted` exists, both the source
> `definition_digest` and the effective `profile_digest` are durable execution
> facts and survive restart unchanged.

The ownership commit is the one boundary that writes them, from the frozen
`ResolvedSubagentSpec`. Recovery restores exactly the committed values and
never recomputes either from the current role definition or the current
resource generation: both are mutable, and a reload that redefines the same
agent name must not relabel an already-committed child. Runtime Client projects
both beside each other for a live child and a recovery-projected one alike.

Only the digests are persisted. The effective Tool, Skill, and extension bodies
they summarize are deliberately not durable: the digest is the bounded
correlation fact.

Excluding the raw payload is the point: two invocations that produce the same
effective profile — no override, and an override restating the defaults — share
one identity, and the authorized Tool and Workflow paths agree for equivalent
inputs. Semantically unordered collections are canonically normalized before
framing; the project instruction chain, whose order is meaning, is framed in
order.

The digest is identity and diagnostic/recovery correlation over an
already-authorized contract. It is never an authorization token: no resolution,
admission, or execution path consults it to decide what a child may do. Runtime
Client projects it beside `definition_digest` as a bounded correlation
identity, never alongside the effective selections themselves.

Child composition and `FrozenSubagentResourceLoader` consume this frozen native
specification. Workspace/Git worktree acquisition supplies physical workspace
ownership only. It never restarts launch resolution, reads role roots, walks an
AGENTS.md chain, discovers Skills, or widens MCP/Python/Tool authority. The child
materializes the frozen extension composition exactly as it materializes every
other frozen decision: it never rereads `rustx.jsonc`, host or project
configuration, or role files to reinterpret which extensions it owns. Skill bodies
retain their established progressive-disclosure semantics.

Schema 8 removes inline role payloads and `instructionsFile`; older runtime
document versions are rejected. There is one authoring path and no compatibility
reader or automatic migration.
