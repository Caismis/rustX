# Fixed scoped Workflow programs

WF-01 (#217) and WF-02 (#218) extend the native Workflow foundation (#83). A registered
Workflow remains one foreground Tool. Configuration explicitly registers
`.agents/workflows/<id>.yaml` and separately exposes it through
`workflows.main`. Profiles must belong to `subagents.workflow`. Files do not
grant admission, and a block never rediscovers capabilities or resources.

## Authoring and lexical scope

A definition contains `description`, `block`, an optional explicit `tools`
admission set, and trusted `timeout_ms` (default 600000). Every block contains
`input` and `output` JSON Schemas, `entry`, `nodes`, and `edges`. Parallel
branches contain an `input` value expression and another `block` of exactly
the same shape. There is no Block node, callable subworkflow, conversation,
or independent job. The node vocabulary is Agent, Tool, Branch, Parallel, Return.

## Fixed native Tool nodes (WF-02)

```yaml
description: Check the project.
tools:
  - {origin: mcp, server_id: 'python:verify-greeting', name: verify_greeting}
timeout_ms: 600000
block:
  input: {type: object, properties: {}, additionalProperties: false}
  output: &findings
    type: object
    properties:
      passed: {type: boolean}
      failures: {type: array, items: {type: object}}
    required: [passed, failures]
    additionalProperties: false
  entry: check
  nodes:
    check:
      type: tool
      selector: {origin: mcp, server_id: 'python:verify-greeting', name: verify_greeting}
      arguments: {type: literal, value: {}}
      result: {type: json, part: 1, schema: *findings}
    done:
      type: return
      output: {type: reference, path: [check]}
  edges: [{from: check, to: done}]
```

Builtin selectors are `{origin: builtin, name: <name>}`; MCP selectors also
require `server_id`. Managed Python uses its existing synthesized
`python:<package>` MCP identity. Selectors must occur in the definition's
explicit `tools` admission set. The compiler checks admission, typed lexical
bindings, object arguments, and the closed result schema. The invoking
resource generation resolves the actual capability and native input schema;
normalization and complete input validation remain authoritative at invocation.
Arguments use only the existing value AST. Selector, mode, identity, approval,
and deadlines are never model input. A model-selectable ordinary leaf is
prepared as foreground without changing its canonical definition.

Workflow authority comes from the invoking attempt's immutable available
capability registrations, **not** its model-visible registry. Source availability,
invalid selectors, changed identity, missing explicit admission and ineligible
leaves fail closed. Available-but-inactive capabilities remain invisible to the
parent model. Admission rejects background-only capabilities, composites,
Workflow dispatch, subagents, execution control and interactive intrinsics.
No rediscovery or current-file lookup can replace a frozen registration.
The workspace/environment is the ordinary invoking Tool context; WF-03 leases
and workspace handoff are not implemented.

`result` projects exactly one zero-based native `ToolExecutionResult.content`
part. `type: json` requires `ToolResultContent::Json` and a declared closed
Workflow `schema`. `type: text` requires a Text part and produces a typed string.
Missing/wrong-kind parts fail; other parts are not retained. Multiple parts
are never concatenated or searched. JSON-looking text is never parsed.
MCP structuredContent is its native JSON part after ordinary content parts;
overflow previews are text, not complete JSON, and fail a JSON contract.
Projection validates the actual value and all WF-01 byte/depth/accounting
bounds before committing the node-local value. Oversized output fails without
a partial local commit or implicit spill-file parsing.

Only native `Success` may become business data: `{passed: false, failures: [...]}`
is valid successful output. Failed, Denied, Cancelled, TimedOut and OutcomeUnknown
remain typed execution outcomes through node, block, run and outer ToolResult.
Parallel retains every keyed child error; OutcomeUnknown dominates the outer
summary, otherwise the first failing definition key supplies its typed status.
Child outcomes are immutable. Diagnostics are bounded without replacing status
or claiming that remote effects stopped.

`ToolInvocationId` separates Agent `ToolCallId` correlation from Workflow's
concrete `WorkflowNodeInstance`. A Tool node creates no Assistant, model turn,
canonical call slot or ToolResult message. Native preparation/start/progress/
settlement/completion facts belong to the Event Journal, never parent history.
The ordinary approval rendezvous gates the exact normalized, validated invocation;
Allow/Deny has no argument-replacement channel. Rejection or denial starts no executor.

The caller-independent foreground owner in `tools::invocation` drives one
physical handle, genuine progress, cancellation, hard/idle deadlines and typed
settlement. The Agent Loop still owns canonical calls, sibling ordering and atomic
ToolResult batches. WorkflowRuntime owns only graph admission, bindings, projection,
local commits and all-settle block/run settlement. Executors own physical work,
cancellation, cleanup and evidence. No graph progresses in an executor.

The outer registration freezes one positive finite hard-only total policy
(`timeout_ms`, maximum 24 hours). Its immutable deadline starts at the outer
native execution-start frontier, after outer permission approval. All nodes,
capacity waits and nested permission waits consume it. Steps and progress never
reset it. Each leaf retains its ordinary frozen hard/eligible idle policy and
also observes ancestor cancellation. No artificial idle heartbeat is emitted.

Observable cancellation before node admission prevents any leaf start. After
approval and the leaf scheduling gate, cancellation is checked again before the
native start fact; construction and first poll retain native cancellation guards.
Once started, cancellation requests physical settlement rather than dropping
the operation. A validated successful node value commits once; the root result
still crosses its own cancellation-vs-terminal frontier before outer publication.
Native terminal facts are last in their scope; one accepted outer call receives
one canonical result. Leaf settlement-control guard expiry means control-plane
failure and OutcomeUnknown, never proof of stop. A native child-scope lease
counts each owned Agent/Tool admission-and-settlement future. The composite
first drains these owners, then starts its own finite 30-second control guard,
instead of racing descendant cleanup with an equal timer. A broken composite
with no children still expires as control failure/OutcomeUnknown. The scope
counter owns no result, graph or executor and emits no heartbeat. Parallel joins
all owned branches, including cancellation and failure paths.

Outer Sequential is call ordering, not a held descendant capacity token. Fixed
leaf invocations use one shared fair read/write scheduling gate: sequential
leaves are exclusive, parallel leaves share it. The outer composite never holds
that gate, nor an Agent registry capacity slot, while awaiting descendants.
Thus capacity one is usable without weakening ordinary outer sibling ordering.

The copyable `greeting_check` example uses an ordinary managed Python tool over
two fixed project checks. Findings are machine-derived, not log-keyword inference;
load/contract failures remain native execution failures. It adds no process
supervisor, verifier service, retry, Review, Loop or dynamic graph.

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
state changes; it never spins, sleeps or fabricates an admission. Watch is
only notification, not ordering authority. At the first unavailable-capacity
decision, the registry inserts the staged child's already allocated native
Subagent ordinal into its waiter map, under the same `RegistryState` mutex
as eligibility and ownership commit. This is the wait-registration frontier.
The smallest **registered** ordinal is the only eligible waiter. Pairwise
relative order remains fixed across wakeups/retries; retries do not create a
new position. A later registration participates by its ordinal but cannot
undo a prior ownership commit. Unregistered preparations and external
identity-allocation timing are not a reserved pending set.

Every ownership attempt rechecks eligibility under the registry mutex.
Ordinary `commit` stays non-waiting: full capacity or an existing waiter
returns `CapacityExceeded`; it cannot steal an eligible waiter's free slot.
Only actual native ownership consumes active capacity. Successful admission,
failure and cancellation remove exactly that waiter's coordination entry;
removal notifies successors even when no child remains active. A subscription
is established before checking state, and its observed version is updated
before (never after) the mutex-protected decision. Capacity release between
registration/check and `changed().await` therefore remains an unseen wakeup.
Neither task wake order nor mutex reacquisition decides eligibility.

Cancellation wakes the same wait and conclusively rolls back the staged
child. Runtime drain cancels registered waiters too. A native task retains
the counted lifecycle admission across commit/rollback; the count is obtained
synchronously before task handoff. Dropping a waiting caller cancels this
operation without dropping the staged child. If ownership already committed,
abandonment requests native cancellation and uses the existing settlement
owner. No waiter guard creates an ownership record or a capacity permit.
The wait is
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
durable contract. WF-02 uses SQLite development schema 25, child IPC 16 and
Runtime Client/TUI 18 for caller-neutral approval identity and typed execution
facts. The event envelope stays version 1 because framing is unchanged. The
client projector explicitly ignores journal-only execution facts pending WF-06;
approval remains on the existing human interaction surface.

Tool/deadline composition is WF-02 (#218). Workspace handoff (#219), Review/ask_user
(#220), Loop (#221), full projection (#222), and reference workflows (#223)
remain outside WF-01/WF-02.
