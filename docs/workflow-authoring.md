# Offline Workflow authoring

See [Workflow admission and Agent selection](workflow-admission.md) for static
capability ownership and atomic disable semantics, and [capability inspection](capability-inspection.md)
for the shared generation projection.

```text
rustx workflow check <id> [--workspace <dir>] [--config <path>]
  [--model <model-name>] [--json]
rustx workflow explain <id> [--workspace <dir>] [--config <path>]
  [--model <model-name>] [--json]
```

Both commands inspect a prospective launch. The identity resolves to the effective User/Workspace `.agents/workflows/<id>.yaml`; it is not an execution entry point.
Unused malformed resources produce bounded diagnostics without preventing unrelated analysis.
Session mutations, probe flags, runtime-state paths and Tool/Skill launch
overrides are rejected by this command grammar.

The version-2 report contains `workflow.id`, `source`, typed `admission`, and an
optional compiled `program` explanation. Argument and authoring errors exit 2.
Well-formed inspection exits 3 because provider execution readiness remains
unresolved. Disabled dependencies produce incomplete validity. Enabled static
admission never grants invocation approval or certifies provider connectivity.

## One authority

1. `launch::analyze` resolves User < Workspace configuration, finite demand and canonical
   resources without capturing credentials or preparing sources.
2. `workflow_resources::load` reads canonical YAML through strict owner-native
   authoring types. `WorkflowProgram::compile` validates graph structure, lexical
   scope, schemas, bindings, finite bounds and retained-value reservation.
3. `WorkflowCatalog::admit_metadata` uses the same dependency traversal as online
   `WorkflowCatalog::admit`, with native leaf metadata and unprepared external
   sources. Agent nodes use shared whole-dimension replacement and the one
   `resolve_agent_profile` semantic boundary. One failed dependency disables the
   entire program; no partial executable program is exposed.
4. The candidate freezes `CapabilityInspection`. The command copies the Workflow
   admission fact from that snapshot. `explain` additionally renders compiler-owned
   graph metadata through `WorkflowProgram::inspect`.

Inspection starts zero nodes, Tools or child Agents. It does not install, run uv,
connect to MCP/provider endpoints, create environments or capture credentials.
External Tool schemas are never fabricated offline. Online runtime generations
may establish readiness through demand-driven ToolSource preparation. Each run
retains its own frozen admitted program after later resource publication.

## Diagnostics and explanation

Disabled admission carries deterministic node paths such as
`block.nodes.inspect.selector` and native `WorkflowDependencyFailure` facts.
Tool reasons distinguish an undefined source, undefined/unprepared
source, preparation failure and exact Tool absent from a ready source. Agent
reasons preserve unavailable Skills/Agents/Workflows and unsupported native
Extension scope. These are typed facts, not conclusions reconstructed from prose.

Compiler failures retain parser coordinates and structural paths independently
of capability admission. A disabled source remains discoverable and inspectable,
but never appears in active model-facing Workflow exposure and cannot execute.

The compiled explanation includes schemas, nodes, sorted edges, bindings, branch
keys, limits and selected profile/Tool references. Prompts/tasks and literal
values are omitted; schema constants, enum contents and annotations are redacted.
The report's 256 KiB bound explicitly marks omitted projections. Editor schemas
continue to derive from the authoritative Rust authoring types.

## File authoring (App Server and Web)

The Web **Workflows** area (Issue #491) authors the same native YAML files that
`check` and `explain` inspect. One Workflow is one file,
`.agents/workflows/<id>.yaml`, in exactly one scope: **User** (shown as
"Global", under the User configuration directory) or one registered
**Workspace**. Rust owns the file's semantics, validation, serialization,
revisions and commits; the browser owns canvas interaction only. There is no
second runtime, no database program definition, no browser YAML compiler, no
persisted React Flow JSON and no draft or publish lifecycle: the file is the only
durable definition.

### Scope and identity

Every operation names an explicit `SourceTarget` (`user`, or `workspace` with its
directory) and a Workflow ID. A listing is exact-scope, never a merged catalog:
the Workspace listing shows only that Workspace's files and marks an ID that
also exists globally as overriding it; the Global listing shows only User files.
The same ID in both scopes is two independently addressable files. A target is
never derived from the focused conversation; Workspace directories come from the
Product Host's registration, and the browser never sends a Workspace path.

IDs follow the native `WorkflowId` rule. Paths are resolved by native code
relative to the scope's admitted directory handle: traversal, absolute paths,
symlinked directories or files and a scope redirected after admission are
rejected, and a Workflow directory holds at most 128 files.

### Reading and invalid files

`workflow/list` returns one bounded summary per file (ID, relative path, exact
revision, byte length, modification time, a description bounded to 512 bytes,
node count and an optional diagnostic). A malformed file is still listed with its
bounded diagnostic and stays openable read-only; it is never hidden or repaired.
`workflow/read` returns the strictly parsed document, its revision and the
vocabulary the editor offers (named Agent profiles and selectable native Tools).
Diagnostics carry the native authored path (`block.nodes.<id>.…`), a category,
static reason and correction text, and parser coordinates; authored values and
parser messages are not copied into them.

Browsers represent JSON numbers as doubles. A file containing an integer whose
magnitude exceeds 2^53−1 is reported with category `unsupported_number` and is
read-only in the Web editor rather than silently rounded.

### Checking

`workflow/check` validates a candidate document without writing it. The report
keeps two facts apart:

- **Structure** — the result of `WorkflowProgram::compile` (graph, lexical scope,
  schemas, bindings, bounds): `valid` with the root outputs, or `invalid` with
  one bounded diagnostic at its authored path.
- **Dependencies** — `not_checked`, `admitted`, `blocked` with typed diagnostics,
  or `unavailable` with a reason, computed by the same metadata admission as
  `rustx workflow check` (`MetadataAdmission::compose`).

With `explain`, a structurally valid candidate also returns the compiled
explanation described below. Checking executes no node, contacts no provider or
MCP endpoint, reads no credential and writes nothing.

### Writing, revisions and conflicts

All writes use `configuration/sourceWrite` with the mutation
`{kind: "workflow", id, edit}`, where `edit.operation` is `create` (a minimal
valid program with a description), `replace` (a complete document), `copy`
(from another file of the same scope at its exact `source_revision`) or
`delete`. Each write carries `expected_revision`: the file's last observed
revision (`sha256:<hex>`), or the listing's `absent_revision` for a file that
must not exist yet. A changed revision is a conflict and nothing is written.

Native code validates the candidate before writing; an invalid candidate leaves
the file unchanged and returns `workflow_invalid` with its diagnostic. A valid
candidate is serialized from the native types, reparsed and compared, and
published by the same staged-file and rename path as every configuration
source. Serialization is normalized: comments and anchors are not preserved,
defaulted fields are written explicitly, and an ordinary control port is
omitted rather than written as `null`. Unknown fields are rejected, so nothing
is silently dropped.

The response distinguishes three outcomes, and the Web editor reports each
separately:

1. **Commit** — the new bytes are durable at the returned revision.
2. **Publication** — the configuration revision the commit produced, observed by
   later Session configuration capture.
3. **Adoption** — whether any Session uses it. Saving never changes a running
   Workflow: each run retains its frozen admitted program, and only Sessions that
   later capture configuration adopt the new file.

A lost acknowledgement (for example a dropped connection after sending) is
**uncertain**, not failed. The editor rereads the file and compares the revision
with what it sent; it never replays the write blindly. A matching file is
reconciled as saved, a different one is reported as changed elsewhere, and the
local document is kept in either case.

### Concurrency guarantee

Writers inside rustX are serialized by the source's advisory writer lock, and the
expected revision is checked under that lock immediately before the rename, so
two rustX writers can never both commit against the same base. Writers outside
rustX (an editor, `git checkout`) do not take the lock; their changes are
detected by revision comparison on the next read or write, but a change that
lands between the final check and the rename is not excluded. Writes are atomic
renames, so a file is never observed half-written.

### Editor-local state

Node positions and the viewport are presentation only.
They are kept in browser `localStorage`, keyed by server, scope, Workflow ID and
block path, and are never written to the file; clearing them only resets the
layout. Unsaved edits live in memory: leaving a dirty editor asks for
confirmation, and there are no persisted drafts.

## Templates and execution invariants

Start with [the three small templates](../examples/local-runtime/workflow-templates/README.md).
All support non-Git operation: every Workflow runs in the shared Session
workspace. Keep the full implementation/review/repair reference stack and its
native conformance scenarios as execution evidence.

Workflow remains a concrete foreground-only Tool with Sequential outer siblings,
no automatic replay, native progression/value/budget/settlement ownership, frozen
admission, typed terminal output and bounded loops. Question,
approval and Review remain distinct. Cancellation, terminal uniqueness and
terminal-last ordering are unchanged. Inspection creates no model/provider
request, process/helper, Python environment, package synchronization, network/MCP
connection, Session, WorkflowRun, interaction waiter/request, runtime-state write, catalog/exposure mutation or resource publication.

## Deterministic regression map

| Contract | Test (short name) and file |
| --- | --- |
| Real template loading, canonical roles, compiler/editor agreement | `cfg237_templates_use_native_loader_compiler_and_editor_schema`, `src/runtime/workflow/tests/templates.rs` |
| Typed Agent execution and terminal uniqueness/ordering | `cfg237_typed_agent_template_executes_native_typed_terminal`, same file |
| Keyed parallel native execution, gated opposite completion orders | `cfg237_parallel_template_preserves_keys_under_reverse_completion`, same file |
| Native Review, exact response, no remaining waiter, terminal last | `cfg237_human_plan_template_uses_native_review_and_settlement`, `src/runtime/workflow/tests/human.rs` |
| Missing/non-admitted roles, invalid binding/type/budget, zero effects, no authority | `cfg237_check_explain_zero_effects_precise_errors_and_authority`, `src/local_runtime/launch_tests.rs` |
| Exact root/nested graph paths through final Diagnostic, zero effects for check/explain | `cfg237_graph_paths_reach_diagnostics_with_zero_side_effects`, same file |
| Edge endpoints/ports, later duplicates, deterministic cycle residual, description and structural node paths | `authored_paths` tests, `src/runtime/workflow/tests/authored_paths.rs` |
| Online schema unresolved versus disabled source | `cfg237_online_schema_unresolved_and_disabled_source_are_distinct`, same file |
| Nested branch schema/scope failures, parser coordinates, compiler agreement | `cfg237_nested_paths_parser_locations_and_compiler_agreement`, same file |
| Output bound and honest omission | `cfg237_workflow_projection_omission_preserves_validity_and_size_bound`, same file |
| Loop body/carry scope, iteration bounds and broken control | `cfg237_loop_and_control_failures_keep_authored_paths`, `src/runtime/workflow/tests/templates.rs` |
| Stable compiled explanation under map reordering | `cfg237_compiled_explanation_ignores_yaml_map_insertion_order`, same file |
| Finite grammar and rejection of execution controls | `cfg237_workflow_grammar_is_finite_and_rejects_execution_controls`, `src/local_runtime/cli.rs` |
| Real binary human/JSON framing, exits, invalid paths and no runtime writes | `cfg237_binary_workflow_commands_share_json_exit_and_read_only_contract`, `tests/process/configuration_commands.rs` |

The existing `cfg235_checked_in_schemas_match_authoritative_generation` drift
test, duplicate-YAML-key compiler tests, CFG-04 redaction/causal-output-bound tests,
and complete native Workflow suites remain authoritative. In particular,
`tests/conformance/workflow.rs` retains the shipped repair, exhaustion, frozen
verification, checker tampering, root human interaction, resource freeze and
non-Git parallel scenarios. TUI opaque forwarding remains covered by
`tui/test/configuration-command.test.ts`.

Offline tests measure the effect-owner entries: process supervision, MCP
connection, provider construction, Session/state, trust, Python preparation,
credentials, WorkflowRun, interaction publication/waiter creation, resource
generation construction, and Tool registration/domain admission mutation. Measurement wraps the synchronous inspection entry point;
no sleep or live provider serves as correctness evidence.

Invocation overrides never change Workflow run identity. Workflow Agent children
own their Conversations and run in the shared Session workspace; a Workflow owns
no workspace resource and contributes no Session deletion blocker. See [capability identity and Session ownership](subagent-resources.md#capability-identity-and-session-ownership).
