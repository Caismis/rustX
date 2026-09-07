# Fixed scoped Workflow programs

WF-01 (#217) extends the native Workflow foundation (#83). A registered
Workflow remains one foreground Tool. Configuration explicitly registers
`.agents/workflows/<id>.yaml` and separately exposes it through
`workflows.main`. Profiles must belong to `subagents.workflow`. Files do not
grant admission, and a block never rediscovers capabilities or resources.

## Authoring and lexical scope

A definition contains `description` and `block`. Every block contains
`input` and `output` JSON Schemas, `entry`, `nodes`, and `edges`. Parallel
branches contain an `input` value expression and another `block` of exactly
the same shape. There is no Block node, callable subworkflow, conversation,
or independent job. The node vocabulary is Agent, Branch, Parallel, Return.

```yaml
description: Return an explicitly projected greeting.
block:
  input:
    type: object
    properties:
      name: {type: string}
    required: [name]
    additionalProperties: false
  output:
    type: object
    properties:
      greeting: {type: object}
    required: [greeting]
    additionalProperties: false
  entry: greetings
  nodes:
    greetings:
      type: parallel
      branches:
        greeting:
          input:
            type: object
            fields:
              person: {type: reference, path: [args, name]}
          block:
            input:
              type: object
              properties:
                person: {type: string}
              required: [person]
              additionalProperties: false
            output:
              type: object
              properties:
                name: {type: string}
                salutation: {type: string}
              required: [name, salutation]
              additionalProperties: false
            entry: done
            nodes:
              done:
                type: return
                output:
                  type: object
                  fields:
                    name: {type: reference, path: [args, person]}
                    salutation: {type: literal, value: Hello}
            edges: []
    done:
      type: return
      output: {type: reference, path: [greetings]}
  edges:
    - {from: greetings, to: done}
```

The first reference component is `args` or a local producing node. Remaining
components are literal object field names; array indexing and field-expression
strings are unsupported. A whole-source reference such as `[greetings]` is
valid. `args` is reserved and cannot be a node id. Node and branch keys use
ASCII letters, digits, underscores and hyphens, with a 64-byte maximum.

A child sees only its projected `args` and its own committed node values.
The parent receives only the child's declared output. Siblings may reuse
local names, but cannot reference each other's private values. Return
completes exactly its owning block. A branch Return does not stop siblings;
the root's completed result is the only candidate for WorkflowRun completion.

The compiler checks each graph for an explicit valid entry, dangling edges,
cycles, reachability, termination, and complete true/false Branch ports.
Ordinary nodes have exactly one unlabelled outgoing edge; Return has none.
At joins, availability is the intersection of every predecessor's available
producers, not their union. A reference crosses only required object fields.
This rejects optional-path values, optional fields and use-before-definition.

## Values and predicates

Values are a closed tagged AST:

| Tag | Payload | Meaning |
| --- | --- | --- |
| `reference` | `path: [args, field]` | Explicit lexical lookup |
| `literal` | `value: <JSON value>` | Literal data, never interpreted |
| `object` | `fields: {key: <value expression>}` | Atomic object construction |
| `array` | `items: [<value expression>, ...]` | Atomic array construction |

An Agent's `input` is a map of names to value expressions. A branch's `input`
and a Return's `output` are each one value expression. Tasks are fixed
strings. There is no interpolation, truthiness, callback, Transform node,
or embedded programming language. Unknown fields/tags and duplicate
construction field keys are rejected. A failing construction commits no
partial local value.

Branch `condition` is also explicitly tagged:

```yaml
condition:
  type: and
  predicates:
    - type: boolean
      value: {type: reference, path: [review, passed]}
    - type: not
      predicate:
        type: equal
        left: {type: reference, path: [review, status]}
        right: {type: literal, value: blocked}
```

`boolean` requires a boolean. `equal` and `not_equal` require matching
string, boolean, or null types and compare their exact JSON values.
`not` takes one predicate; `and` and `or` take nonempty predicate lists and
short-circuit in definition order. All operands are statically checked,
including short-circuited ones. Numeric predicates, ordering comparisons,
object/array equality, and all implicit coercions are unsupported. Integer
and number data can still be bound, constructed and returned.

## Conservative schema proof

Every schema declares one string `type`: object, array, string, boolean,
null, integer, or number. The closed recursive keyword vocabulary is
`type`, `properties`, `required`, boolean `additionalProperties`, one-schema
`items`, `enum`, and `const`. Numeric `const`/`enum` refinements are rejected:
the current runtime validator's numeric-const approximation is unsuitable
for a sound finite-value static proof. Numeric values in ordinary typed
fields remain supported.

Unsupported keywords, unions/combinators, references, tuple schemas,
length/range constraints and unprovable relationships fail compilation.
Required fields must have schemas. Object compatibility proves required
fields, optional declared fields, and additional-property restrictions.
Array compatibility proves one element contract; constructed arrays require
a provable common element contract. Integer producers may bind to number
consumers, but number producers are not generally proven integer. Exact
literal values and supported finite enum/const producers are checked against
the consumer's schema.

This is a conservative structural proof, not arbitrary JSON Schema
subtyping. Full runtime schema validation remains authoritative for Workflow
input, projected block input, native child output, joined Parallel output,
and every block Return. Child output is validated both at native settlement
and against the consuming node's immutable contract.

## Identity and lifecycle ownership

`WorkflowProgram` contains one immutable root `WorkflowBlockProgram`.
Every compiled branch holds an input expression and a `WorkflowBlockProgram`.
`WorkflowRuntime::execute_block` is the sole root/branch execution and
lifecycle entry point; it owns private input/local-value lifetime and invokes
the same node dispatch for every scope. No one-Agent Parallel executor or
legacy grammar remains.

`WorkflowDefinitionPath` contains the configured Workflow id and alternating
Parallel-node/branch keys. A `WorkflowRunId` contains the conversation, native
admitted `AttemptId`, and a WorkflowRuntime-owned invocation ordinal.
Cloned runtimes share the ordinal allocator, including across resource reload.
Recovery creates a fresh native attempt identity rather than resuming a run.
`WorkflowBlockInstance` combines
that run, the static path and structural invocation components.
`WorkflowNodeInstance` adds the local node and visit ordinal. WF-01 uses zero
invocation/visit ordinals; later Loop iterations can supply fresh components
without cloning static definitions. No identity uses prose, PID, provider
text or completion order. The outer `ToolCallId` appears on WorkflowStarted
only as model correlation. Registry terminal routing and node facts carry
the concrete instance; child ToolCall correlation is a bounded hash of it.

Parallel freezes its keyed branch set at node admission. Every ordinary
branch failure is collected while all branches continue to settle. Join
result insertion and failure aggregation use definition-key order. Branch
readiness does not select result keys or change count/data admission limits. Node progress,
native process staging, external capacity contention and event timestamps
are not deterministic timing guarantees. Branch-local events naturally
reflect their actual execution; the join fact records sorted success/failure
keys after all branches settle.

Native capacity is checked by `SubagentRegistry` at ownership commit.
`commit_waiting` retains the actual staged child and watches native registry
state changes; it never spins, sleeps or fabricates an admission. Cancellation
wakes the same wait and conclusively rolls back the staged child. The wait is
bounded by the foreground execution's existing finite deadline/cancellation
authority. Zero configured capacity rejects rather than waiting forever.
No block owns a capacity permit. A parent block waiting for descendants
therefore cannot consume the permit they need, including at capacity one.

The node admission frontier is the synchronous cancellation check and
aggregate execution-count reservation before child preparation. Observable
cancellation before that check prevents the node, child admission and model
start. Native preparation and commit independently recheck cancellation.
After native admission, the Agent waiter requests native cancellation and
awaits native settlement. Parallel owns all its branch futures through the
join, including error and cancellation paths; none is dropped to fabricate
settlement. The order is native child settlement, owning block terminal,
root block terminal, WorkflowRun terminal. Terminal outcomes are unique.

## Aggregate budgets and retirement

| Bound | Limit | Admission/commit owner |
| --- | --- | --- |
| YAML source and serialized program | 512 KiB each | Loader read; compiler before block traversal |
| Static nodes, including every nested graph | 256 | One compiler counter shared by all blocks |
| Nested Parallel depth | 8, root depth zero | Recursive compiler admission |
| Branches per Parallel | 32 | Compiler; aggregate nodes still apply |
| Reference path | 32 components, 64 bytes each | Expression static validation |
| Native conversation/attempt identity components | 256 bytes each | Run admission; static paths have at most 16 bounded keys |
| Expression/predicate serialized size | 64 KiB each | Compiler; aggregate program bytes still apply |
| Expression/schema/value depth | 32 | Compiler; runtime value commit |
| One constructed/retained value | 64 KiB | Construction before commit and local reservation |
| Admitted nodes | Program's total static count, at most 256 | WorkflowRun counter at node frontier |
| Admitted Agent executions | At most 256 | Same run-owned reservation, before native preparation |
| Retained Workflow inputs, locals and exports | 4 MiB | Compiler proves whole-program reservation; run charges actual bytes before local commit |
| Native/branch failure diagnostic text | 1 KiB per diagnostic | Clamp before retention/aggregation; preserve every outer branch key |

Count reservations are conservative and never refunded or reset on child
entry, failed preparation or scope completion. Private block inputs and
locals have one accounting reservation. A Return obtains an export
reservation before reporting success; private reservations retire on block
exit. Completed branch exports remain charged until the parent join consumes
them. The joined object is validated/reserved before insertion into parent
locals. Conservative overlap during export/join transfer is counted too.
Rejected byte reservations change no counters or local values.

Data admission is independent of sibling completion timing. Before execution,
the compiler sums conservative maximum sizes for every block input, producing
node local, and Return export across the entire nested program. Closed
objects, booleans, nulls and supported finite schemas give tighter bounds;
otherwise a value reserves the full 64 KiB maximum. Mutually exclusive paths
are conservatively counted together. A program exceeding 4 MiB is rejected
before any work starts, even if a particular input might have fit. The run
owns that frozen reservation; actual retained bytes cannot exceed it. Private
retirement releases actual bytes without redistributing admission authority
between siblings. This avoids completion-order-dependent budget failures.

Failure text is separate from business JSON. The 256-node aggregate bound,
64-byte keys, bounded instance paths, and 1 KiB diagnostic clamp also bound
all pending failure records, formatted joins and their construction overlap
to less than 2 MiB per run. Including these diagnostics, retained Workflow
data has a conservative 6 MiB ceiling. This excludes the separately bounded
immutable program and native child-owned resources. Journal copies are
observation evidence under their existing durable owner.

The registry transfers a successful native output once and clears its live
copy. The existing durable output fact remains bounded evidence, not live
block state. Native child transcripts, process resources and workspace
ownership remain under their existing native owners and bounds.

## Freeze, history and observation

The registered Tool captures an immutable program. The invoking attempt
supplies frozen resource, profile, capability and model authority. Later file
edits or resource publication affect future attempts only. Entering a child
block neither reloads files nor widens authority. Child success still requires
the sole native `workflow_output` protocol; WorkflowRuntime makes no model
requests and orchestration stays outside ToolExecutor.

Canonical history remains main ToolCall → native Workflow/child execution →
one main Tool result. Private values and child transcripts are never appended
to the parent's canonical conversation. Workflow Tools remain foreground-only,
outer siblings remain sequential, and no replay/resume is introduced.

The Event Journal adds bounded block/node start and terminal facts and typed
instance associations. It remains best-effort observation for ordinary
Workflow lifecycle; the native child output/terminal pair retains its atomic
durable contract. SQLite development schema 24 replaces 23. The event
envelope stays version 1 because its framing is unchanged. Child IPC and
Runtime Client/TUI wire contracts are unchanged; the client projector explicitly
ignores these new journal-only facts pending WF-06.

Tool/deadline composition (#218), workspace handoff (#219), Review/ask_user
(#220), Loop (#221), full projection (#222), and reference workflows (#223)
remain outside WF-01.
