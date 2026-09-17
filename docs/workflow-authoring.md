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

1. `launch::analyze` resolves authorized configuration, source policy and canonical
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

## Templates and execution invariants

Start with [the three small templates](../examples/local-runtime/workflow-templates/README.md).
All support non-Git operation. Candidate isolation is explicit, not a universal
prerequisite. Keep the full implementation/review/repair reference stack and its
native conformance scenarios as execution evidence.

Workflow remains a concrete foreground-only Tool with Sequential outer siblings,
no automatic replay, native progression/value/budget/settlement ownership, frozen
admission, typed terminal output, candidate identity and bounded loops. Question,
approval and Review remain distinct. Cancellation, terminal uniqueness and
terminal-last ordering are unchanged. Inspection creates no model/provider
request, process/helper, Python environment, package synchronization, network/MCP
connection, Session, WorkflowRun, interaction waiter/request, worktree/candidate,
runtime-state write, catalog/exposure mutation or resource publication.

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
verification, candidate tampering, root human interaction, resource freeze and
non-Git parallel scenarios. TUI opaque forwarding remains covered by
`tui/test/configuration-command.test.ts`.

Offline tests measure thirteen effect-owner entries: process supervision, MCP
connection, provider construction, Session/state, trust, Python preparation,
credentials, WorkflowRun, interaction publication/waiter creation, workspace
acquisition, resource generation construction, and Tool registration/domain
admission mutation. Measurement wraps the synchronous inspection entry point;
no sleep or live provider serves as correctness evidence.

Invocation overrides never change `borrowed_from`, Workflow run identity, or
physical disposal authority. Borrowing Agent children own their Conversations,
while a shared Workflow-owned worktree contributes exactly one Session deletion
blocker. See [capability identity and Session ownership](subagent-resources.md#capability-identity-and-session-ownership).
