# Offline Workflow authoring

```text
rustx workflow check <id> [--workspace <dir>] [--config <path>]
  [--models <path>] [--model <provider/model>] [--json]
rustx workflow explain <id> [--workspace <dir>] [--config <path>]
  [--models <path>] [--model <provider/model>] [--json]
```

Both commands inspect the prospective launch environment. `<id>` is a configured
`workflows.definitions` identity resolved to the trusted workspace's
`.agents/workflows/<id>.yaml`. It is not a path, discovery request, or execution
entry point. The configured environment is validated before the selected program
is projected, so another invalid registered resource can prevent inspection.
Trust/Session mutation, preparation/probe flags, runtime-state paths, and Tool or
Skill selection overrides are rejected. `--help` contains the grammar.

Both reuse CFG-04's version 1 report. `--json` emits one JSON object; human output
adds a status line and formatted JSON. Argument or static errors exit **2**. A
valid static structure exits **3**, because readiness remains `unresolved`.
Missing online facts, inert dependencies or untrusted resources produce
`validity: incomplete`, also exit **3**. Neither command issues a readiness
certificate. File discovery, registration, role admission, main-model
admission/exposure and concrete runtime admission are separate facts.

## One authority

The shared pipeline is:

1. `launch::analyze` resolves bounded authorized local configuration, model
   selection, source enablement/trust and canonical Subagent resources.
2. `workflow_resources::load` reads configured identities. Serde YAML, wrapped
   by `serde_path_to_error`, deserializes the authoritative `WorkflowDefinition`.
   Strict unknown fields and unique graph keys remain parser-owned.
3. `WorkflowProgram::compile` validates graphs, lexical scopes, schemas,
   bindings, admitted profiles, finite bounds and retained-value reservation.
   Its immutable program remains the executor's sole program type.
4. `WorkflowCatalog::inspect_metadata` uses the native selector resolver and
   genuine metadata. Prospective analysis and runtime resource admission share
   this path. Typed dependencies retain known, inert, unavailable and unresolved
   states instead of dropping unavailable-source facts.
5. `WorkflowProgram::inspect` projects compiled facts; the local report adds
   resolution provenance and configured source/profile policies.

Static compilation ends before any executor is constructed, resource generation
published or execution authority granted. Invocation still freezes the attempt's
admitted roles/capabilities. Native preparation normalizes and validates concrete
Tool arguments; execution validates concrete values. An online-only Tool schema
is never supplied or assumed compatible offline. Provider behavior, source
availability, human decisions and candidate acquisition remain runtime facts.
The offline owner accepts no executor, interaction/workspace manager, credential
snapshot or publication handle. There is no alternative parser/compiler/executor.

## Diagnostics and explanation

Compiler context preserves nested paths such as
`block.nodes.check_text.branches.clarity.block.nodes.assess_clarity.input.text`.
Loop bodies use `.body`; schema recursion uses `.properties.<name>` and `.items`.
Parser failures retain typed container paths and YAML line/column when available;
unknown failing field names are redacted using the generated structural vocabulary.
Semantic
diagnostics have compiler-authored paths and do not invent parser coordinates.
Categories distinguish `workflow_language`, `resource_missing`,
`resource_not_admitted`, `dependency_inert` and `unresolved`. Resolution stops at
its first authoritative failure. Untrusted contents are not read or compiled.

`check` gives registration/admission/exposure, role provenance and dependencies.
`explain` additionally gives compiled identity/digest, explicit validation stage,
root/block schemas, entries, nodes, deterministic edges, bindings and reference
paths, keyed branches, loop limits, selected profiles/tools, timeout, workspace
request and candidate-handoff expectation. Role facts include canonical selected
source/layer/overridden source, model, configured timeout, Tools and workspace
policy. Agent parallel capacity and native caps are separate from compiler-derived
conservative step, Agent-run and retained-byte reservations. Agent-run bounds are
**not** provider-request predictions. Branch outcomes and actual loop iterations
are never predicted.

Blocks/nodes/roles use sorted identity maps; edges are compiler-sorted by port
then target. Branch keys are independent of completion order. Prompts/tasks and
literal values are omitted. Schema `const`/`enum` contents and annotations have
explicit redaction markers: these are schema projections, not replacement editor
schemas. CFG-04 redaction and its 256 KiB output bound apply. Oversized projections
are explicitly omitted without changing validity/readiness or losing the causal
diagnostic. Editor schemas continue to derive from authoritative Rust types.

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
runtime-state write, registration/exposure mutation or resource publication.

## Deterministic regression map

| Contract | Test (short name) and file |
| --- | --- |
| Real template loading, canonical roles, compiler/editor agreement | `cfg237_templates_use_native_loader_compiler_and_editor_schema`, `src/runtime/workflow/tests/templates.rs` |
| Typed Agent execution and terminal uniqueness/ordering | `cfg237_typed_agent_template_executes_native_typed_terminal`, same file |
| Keyed parallel native execution, gated opposite completion orders | `cfg237_parallel_template_preserves_keys_under_reverse_completion`, same file |
| Native Review, exact response, no remaining waiter, terminal last | `cfg237_human_plan_template_uses_native_review_and_settlement`, `src/runtime/workflow/tests/human.rs` |
| Missing/non-admitted roles, invalid binding/type/budget, zero effects, no authority | `cfg237_check_explain_zero_effects_precise_errors_and_authority`, `src/local_runtime/launch_tests.rs` |
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
