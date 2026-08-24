# Runtime resources and executable authority

`RuntimeResourceSnapshot` is the immutable process-local owner of loaded
resource-derived authority. One generation contains the ordered project
context files and concatenated bytes, the compact Skill catalog and discovered
Skill source identities, the agent profile and extension System Sections, and
the compatible immutable `CapabilitySnapshot` containing the Tool definitions
and executors.

This object is not a conversation fact and is not a durable resource database.
`RuntimeResourceRevision` is only the process-local identity recorded with a
request. `ContextGeneration` continues to describe accepted context proposal
ownership and is not a resource lifetime.

## System authority

Canonical history contains only User, Assistant, and Tool messages. Project
instructions, child agent profile/persona, certified extension instructions,
and compact Skill guidance are typed request-time System Sections in this
total order:

1. `CoreRuntimeIdentity`
2. `AgentProfile`
3. `WorkspaceInstructions`
4. `CertifiedExtension` sorted by stable logical identity
5. `NativeCapabilityGuidance`

Their rendered value, `ModelRequest.effective_system_prompt`, is the only
System authority. OpenAI Chat Completions, OpenAI Responses, and Anthropic
adapters translate that value to their protocol field when non-empty and send
no System instruction when it is empty. They never reconstruct authority from
history.

Every `RequestSnapshot` stores the exact rendered prompt, exact ordered
sections, Tool definitions, capability revision, model state, and resource
revision by value. Historical reconstruction reads that snapshot and its
historical Surface revision only; it never reruns discovery or extension
logic.

Context continuity is a property of primary model requests: each primary
request combines its selected canonical Surface revision with the exact
attempt-pinned System and Tool authority. A maintenance summary invocation is
not a second continuity history, and merely retaining historical messages does
not make their former executable authority current.

## Project instruction discovery

At runtime creation or explicit reload, applicable directories are traversed
deterministically from filesystem root to the workspace/cwd. At most one file
is selected per directory with this precedence:

1. `AGENTS.override.md`
2. `AGENTS.md`
3. `AGENTS.MD`
4. `CLAUDE.md`
5. `CLAUDE.MD`

Selected source paths and UTF-8 contents retain that root-to-leaf order and
are concatenated deterministically. Discovery never runs during ordinary
request assembly.

## Lifecycle and external edits

| Operation | Resource behavior |
| --- | --- |
| runtime creation / cold reopen / resume | discover once and publish generation 1 |
| ordinary primary request or tool continuation | reuse the attempt-pinned generation |
| automatic overflow compaction or manual `/compact` | reuse frozen inputs; no discovery |
| Runtime Client detach/reattach | no discovery |
| fork/clone/tree historical projection | select history only; no discovery |
| explicit `/reload` or runtime reload API | prepare a complete candidate and atomically publish it for future attempts |

Before reload or cold recreation, edits to project instructions, Skill
addition/removal/rename/frontmatter, and extension/Tool configuration have no
effect. A Skill catalog freezes only compact metadata and the discovered host
path/source identity. An already-discovered `SKILL.md` remains ordinary file
content: native Read observes its current body at execution time and returns
the normal read error if the file disappeared. Reload never rewrites an old
ToolResult.

## Admission, pinning, and reload

Attempt admission and reload share one synchronization boundary. Admission
pins one `Arc<RuntimeResourceSnapshot>` and acquires a lease for that exact
snapshot's compatible `CapabilitySnapshot`; every model turn and tool
continuation in the attempt uses that pair.

Reload closes a narrow admission gate, verifies there is no active attempt,
pending Question/Approval interaction, or manual compaction, and retains one
counted lifecycle admission while releasing the synchronous state lock before
asynchronous discovery/preparation. On success it commits the complete
capability candidate and publishes the complete resource generation before
reopening admission. On failure it keeps the old pair and reopens the gate.
Thus an admitted attempt lands wholly before or wholly after reload and cannot
observe mixed project, Skill, extension, or Tool generations. Reload emits no
canonical message.
Dropping/cancelling a reload while preparation is in flight releases the gate
without publishing its candidate. Explicit reload inputs republish their Tool
registry even when schemas are unchanged, because executor configuration is
not encoded in model-facing definitions.

## Capability publication and MCP physical ownership

`RuntimeResourceSnapshot` is the live product resource authority. Before a
conversation runtime claims a `CapabilityCoordinator`, the coordinator may be
used as a standalone prepare/commit owner during composition. The claim then
transfers one private publication authority to `ConversationRuntime`; the
ordinary coordinator `commit` API is rejected after that point. A live
capability change must therefore come through `reload_resources()`.

Reload holds the runtime admission lock while its runtime-owned publication
operation commits the matching `CapabilitySnapshot` and assigns the new
`RuntimeResourceSnapshot`, then clears the reload gate. Attempt admission
uses that same lock, so no attempt can enter between the capability swap and
resource assignment. The capability snapshot also carries the immutable MCP
lease authority for its own physical generation. An attempt admitted with
resource generation A can therefore acquire only A's MCP leases, even if B
becomes coordinator-current later.

Each prepared candidate owns every MCP runtime it connects. The capability
commit linearization transfers those runtimes into the published generation;
rejected, stale, cancelled, or dropped candidates retire and settle their own
physical runtimes. A published generation becomes retired when superseded,
but explicit attempt/background leases keep it alive until those owners
settle. Successful close proves physical reclamation and removes the
generation from the retirement registry. `McpError::PhysicalSettlement` means
that proof was not established: the registry retains the generation and the
failure as authoritative evidence, the runtime fences healthy admission via
its existing drain lifecycle, and shutdown reports the failure. This
post-publication settlement failure is distinct from a pre-publication reload
failure: the new generation remains the logical current authority in the
former case.

## Historical observations and lineage

Agent Status is an append-oriented Context Observation Fact. Multiple status
observations may remain in their canonical order until normal Surface
replacement/compaction removes them; old status is never scanned to reconstruct
live state.

Fork, clone, and tree operations project a historical prefix. At a selected
human User boundary, the selected message and context/status belonging to that
old turn and later are excluded while earlier status facts may remain.
Destination-unique `MessageId` and `ToolCallId` values are remapped. Retained
`ToolExecutionId`, `SubagentId`, and background identifiers remain opaque
history and never reacquire live owners. Future destination requests use the
destination runtime's current resources and current Session intent.
