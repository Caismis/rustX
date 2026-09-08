# Fixed scoped Workflow programs

WF-01 (#217), WF-02 (#218), and WF-03 (#219) extend the native Workflow foundation (#83). A registered
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
or independent job. The node vocabulary is Agent, Tool, Branch, Parallel, Review, Return.

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
Workflow dispatch, subagents, execution control and todo. Ordinary ask_user is eligible.
No rediscovery or current-file lookup can replace a frozen registration.
Without a run `workspace` binding, Tools use the ordinary invoking context.
With a binding, every Tool consumes the exact native-authorized candidate as
an exclusive validation consumer, under the contract below.

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

All native invocation Journal facts are best-effort observations. Approval alone
requires a durable exact-preparation boundary: InteractionRequested commits its
immutable subject through durable interaction authority before a human can see
the prompt. It does not depend on a Prepared execution event. A concrete node
visit cannot acquire a replacement approval subject; another visit needs its own
interaction. Publication failure is interaction authority loss, not human Denied.

The capability plane owns `ToolSelector` and source-qualified resolution; neither
Workflow nor generic Tool selection depends on the Subagent feature's resolver.
Registration/admission owns Leaf/Composite policy; executors cannot declare a
Workflow timeout. Ancestor deadlines propagate through the native lifecycle's
absorbing cause view before cancellation is signalled. The descendant records
`Deadline(Hard)` and, with confirmed cancellation, TimedOut. It never derives a
user-cancellation fact from the attempt's default reason.

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

Downstream admission evaluates dependencies, borrows the exact candidate when
required, then observes cancellation and checks/commits node and Agent counts in
one synchronous run-budget critical section immediately before
`WorkflowNodeStarted`. Stale-candidate failure, pre-start cancellation and budget
rejection commit no count. Once committed, counts are not refunded for subsequent
preparation/execution failure.

Workflow owns pre-start candidate access in one async result/cleanup scope. Every
error after borrow returns through its finalizer unless ownership has transferred
to the native consumer. The finalizer uses unchanged-validator `finish(true)`:
it clears native admitted ownership and preserves the version when no source
changed; real interference retains the existing conservative validation behavior.
Tool setup borrows that owned slot through physical settlement; Agent setup leaves
it there until native child preparation takes it. Pure consumers retain it through
consumption. Zero-start cancellation emits no node start/settled facts and cannot
alone manufacture abandoned-user/NestedContainment resource state. Review retains
its separate existing CandidateFreeze acquisition path, without a duplicate borrow.

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
durable contract. WF-03 uses SQLite development schema 28, child IPC 18 and
Runtime Client/TUI 20 for candidate resource ownership and borrowed child workspace
facts. The event envelope stays version 1 because framing is unchanged. The
client projector explicitly ignores journal-only execution facts pending WF-06;
approval remains on the existing human interaction surface.

Review/ask_user is described in WF-04 below. Loop (#221), full projection (#222),
and composed reference workflows (#223) remain separate slices.

## Run-scoped candidate workspace (WF-03)

Trusted definitions can add this run resource, never a graph node:

```yaml
workspace:
  require_clean_parent: true
```

Omission means no new Git requirement, discovery or acquisition. An empty
`workspace: {}` selects the same strict isolated-worktree default. Explicit
`false` permits a dirty parent but does not copy arbitrary parent bytes.
The native `.worktreeinclude` policy is the only authorized ignored-file
overlay; its frozen source and safe materialization rules remain unchanged.

All Agent profiles in all branches must already resolve to exactly the same
`GitWorktree { require_clean_parent }` policy. Shared profiles and mismatched
cleanliness policies fail before acquisition or child/Tool side effects.
Candidate Agents cannot select MCP (including managed Python) or nested
`subagent`/`execution` orchestration: their existing bindings cannot safely
promise candidate cwd or descendant access. These combinations are rejected,
not stripped from a profile. Tool executors declare the shared `WorkspaceUse` contract:

- `ConsumesProvided`: uses the supplied workspace as cwd/file authority. Native
  filesystem Tools and Bash acquire the exact candidate validation borrow.
- `Independent`: consumes no workspace authority. Native `ask_user` runs unchanged
  without acquiring, holding or validating CandidateScope access, and its result
  gains no candidate applicability merely because the run owns a workspace.
- `Incompatible` (the default): external/fixed-cwd executors cannot safely honor
  the binding and fail admission before execution. Current MCP executors use this.

No frozen policy is reinterpreted against a different checkout. A pending
Questionnaire cannot block a writer solely because the run owns a candidate;
source mutation cannot turn its independent response into candidate failure.

Agents are exclusive source-writing borrowers. Every workspace-consuming Tool
is an exclusive **validation** borrower, regardless of its name. This is not
a read-only sandbox: a test may write, but then cannot certify its input.
All candidate consumers serialize, including consumers in Parallel branches
and Tools whose ordinary scheduler permits parallel execution. There is no
parallel-reader claim. Candidate access is acquired before child capacity or
Tool scheduling. No borrower waits for another candidate-consuming child;
nested orchestration is rejected before execution. Ordinary supervised OS
descendants stay inside their existing physical containment boundary.

### One owner and explicit frontiers

```text
Workflow run (logical candidate scope)
  -> runtime::workspace::CandidateScope (native owner)
      -> one WorkspaceLease
          -> one WorkspaceAccess for an exact node instance
              -> Agent process / native Tool / supervised descendants
```

`runtime::workspace`, moved from `runtime::subagent::workspace`, is the sole
Git/resource owner. `WorkspaceUse::Owned` transfers a one-shot lease to the
native process driver; `Borrowed` transfers access only. There is no second
Workflow Git manager. WorkflowRuntime holds the logical scope and never
runs Git. Candidate access changes cwd, not instructions, Skills, model,
profile, capability allowlists, ToolVersion/MCP identities, approvals or
deadlines. Downstream Agents receive explicit values and their own context,
not the preceding Agent's transcript.

The linearization points are:

1. Native `acquire` freezes repository identity, logical relative scope,
   baseline, cleanliness/overlay facts and deterministic physical ownership.
   A staged failure settles that same lease; dirty or unknown work is preserved.
2. `retain_for_run` transfers the sole lease into native scope state. Required
   durable `WorkflowWorkspaceOwned` publication commits run admission. Failed
   publication or observed cancellation settles staging; no node starts.
3. `CandidateScope::borrow` takes the exclusive native mutex, validates the
   exact run/reference/Git ownership and content, installs mutation observation,
   checks cancellation, then marks access admitted. Paths and references alone
   cannot construct access. Cancellation while waiting starts no new work.
4. Native physical settlement precedes `WorkspaceAccess::finish`: Agent reap
   and nested-anchor containment, or native Tool settlement. Only then is
   content inspected and the current version published. Dropping an access
   future leaves it admitted/unresolved, not released.
5. Clearing admitted state and releasing the guard makes the next borrower
   eligible. Unknown containment poisons the scope; no next writer is granted.
6. `CandidateScope::settle` waits for access, revalidates final content, then
   consumes the lease through the same native settlement on every terminal
   path. Settlement is absorbing. Inspection/removal never precedes physical
   containment. A run-scoped native settlement anchor also covers acquisition
   and final cleanup, so outer cancellation cannot outrun this boundary.
7. Exact native proof plus settled ownership authorizes disposal. Automatic
   disposal removes only clean, unchanged managed work. Changed work is retained;
   unknown ownership is durably unresolved. Explicit disposal takes run identity
   and authoritative journal facts, never a caller-supplied path, branch or SHA.

Cancellation is intent, not rollback. Once physical work starts, cancellation
propagates and settlement is still awaited. Cancellation observed at final
run commit prevents a success outcome but cannot reverse already completed
physical settlement. No terminal path automatically commits, integrates,
resets, stashes or force-removes dirty source.

### Candidate content and interference

`CandidateReference { run, version, content }` is a historical native fact,
not an access token. The SHA-256 content identity covers frozen repository,
logical/physical workspace and baseline facts, current HEAD, exact stage/index
entries, and the repository-wide union of tracked, HEAD-tree and non-ignored
untracked paths. Length-framed names and bytes distinguish deletions, staged
versus working-tree state, regular-file executable modes and symlink target
bytes (links are not followed). Thus identical HEAD with different dirty bytes
is not the same candidate. Ignored untracked build/cache/runtime files are
excluded, including ignored overlay files; tracked files remain included even
if an ignore rule matches them.

Unmerged index stages, gitlinks/submodules, sparse/assume-unchanged entries,
special files, unsafe paths and oversized content fail closed. The bounded
algorithm permits at most 100,000 paths, 16 MiB Git listings and 256 MiB source
bytes; it is not a source archive. Native ownership is re-proved around two
matching content scans using stable directory-relative, no-follow reads.

The runtime prevents overlapping **runtime-owned** writers. It does not
sandbox arbitrary host processes. Every access revalidates exact source facts;
Linux inotify/macOS vnode observation detects observed source/control writes
during validation, including writes followed by restoration of identical bytes.
Darwin attribute-only notifications exclude access-time-only bookkeeping when
all other recorded attributes are unchanged. This filters read noise; it is
not source-equality proof. Content/index/mode fingerprints and independent
data-write notifications remain required.
Observation loss/overflow or invalid ownership fails closed. macOS directory
notifications may conservatively invalidate a check for directory changes.
Kernel notification semantics are not universal external-process isolation:
privileged interference, remote filesystem changes, and writes outside kernel
notification coverage are outside this guarantee. Timestamps are never proof.

A proven validation mutation publishes a new version and fails applicability,
even if final bytes were restored. Lost directory coverage instead leaves
currentness unresolved and cannot certify an unchanged candidate. The durable `WorkflowCandidateInvocation`
correlates the actual native outcome with the input reference and an explicit
`candidate_unchanged` fact; native Success alone is insufficient. Historical
checks never authorize a later version. External net changes between borrowers
or before final settlement invalidate admission/final reference and preserve
the workspace conservatively. Model text is not native verification evidence.

### Committed-value applicability and consumption

The interpreter retains `CommittedValue { value: Value, candidate:
Option<CandidateReference> }`. This metadata is internal, never an authored
JSON field. A successful candidate Tool projection carries the exact input
reference returned directly by native invocation after physical settlement and
source/mutation verification. Successful candidate Agent structured outputs
carry the exact post-node reference returned by `WorkspaceAccess::finish(false)`
after child and nested physical settlement. `WorkspaceUseSettlement.candidate`
passes this fact through `PhysicalSettlement.candidate` to the registry's
process-local `WorkflowAgentOutput.candidate`, then Agent settlement commits it
in `CommittedValue`. No candidate identity enters authored JSON or durable child
output. With no mutation A stays A; a writer transforming A into B binds its
output to B. Inspection failure or unresolved containment supplies no successful
candidate-bound local output. A machine-review Agent's `passed=true` for A
becomes stale after a later writer produces B, just like a Tool check.
Literals and external run arguments are unbound.
References retain applicability, including field selection; objects and arrays
merge their dependencies. Parallel inputs, branch Returns and keyed exports
retain the same metadata. Mixing different references fails explicitly; there
is one run candidate, not a provenance graph.

Branch predicates, Return, derived Tool arguments, derived Agent inputs and
Parallel block input/export commits check applicability through the live
`CandidateScope::assert_current`. It validates the run and exact current
reference and fails closed for unresolved/released state. It grants no access
and changes no candidate. Tool/Agent admission additionally checks the expected
reference while acquiring exclusive access, after queued writers settle.
Journal correlation records history and is never queried for this decision.

The linearization sequence is:

```text
A -> exclusive Tool access admitted
  -> native check executes: passed=true
  -> physical Tool settlement -> source/mutation verification
  -> committed local value carrying A
  -> later writer admitted -> physical writer settlement -> version B committed
  -> Branch/Return attempts to consume A -> Workflow InvalidValue (stale reference)
```

The native Success remains historical Success. Applicability rejection is a
Workflow-domain failure; it does not rewrite the historical invocation outcome.
Denied, Failed, Cancelled, TimedOut and OutcomeUnknown retain their native
meaning. Unknown physical settlement never releases borrower ownership.

Directory watch admission enumerates the existing candidate tree using
no-follow, descriptor-relative traversal, including empty and ignored
directories. The fixed limits are 100,000 enumerated entries, depth 64 beneath
the root, and 100,000 total source/control/directory watch candidates. Allocation
or enumeration failure rejects admission. Existing source/control files and
necessary control ancestors are also covered. This is a bounded observation
interval, not an unbounded recursive watcher.

Linux inotify observes child names at each admitted directory. Creating or
moving in a new directory invalidates coverage immediately when notifications
are drained; its descendants cannot silently certify unchanged content.
Ignored file activity may be excluded using Git source policy. macOS vnode
observes existing files and directories but cannot name a changed child;
directory-entry changes conservatively invalidate coverage, even in ignored
caches. Directory watch loss, revocation and Linux queue overflow fail closed.
Neither platform uses timestamps to certify equality or claims arbitrary-host
sandboxing.

### Terminal handoff and resource persistence

Successful candidate runs return `{output, workspace, candidate}`. Failed or
cancelled runs attach the same bounded structured workspace settlement and
optional final candidate reference to their native Tool result, even without
business Return. Unresolved ownership has no final valid reference. Required
resource journal facts are separate from best-effort execution observation.

`WorkspaceManager::inspect_workflow_workspace` reads historical settlement
and separate disposal status; `dispose_workflow_workspace` accepts only the
run identity and conversation store. Its durable intent and physical phases
use the existing native disposal primitive. One-shot proof/disposal entry points
accept only `SubagentId`, so they cannot bypass Workflow journal/content authority.
Repeated disposal is idempotent
and retained source bytes are revalidated before deletion, including retries
that have not yet removed the worktree. A changed handoff fails closed
and cannot rewrite the Workflow terminal outcome. Recovery exposes retained
or unresolved resource facts, never resurrects borrowers, nodes or execution
authority. No Workflow resume or full inspector UI is introduced here.

Unresolved candidate state stores a typed reason and a bounded detail.
`NestedContainment` means a physical user/descendant remains unproven: active
process-local ownership stays registered and Git-only disposal is rejected,
including after reopening. `PhysicalSettlement` means physical users have
settled but final hashing, Git inspection or mutation coverage is uncertain.
At absorbing run settlement the native lease preserves the workspace and
retires its active registration before returning this fact. The existing exact
native re-proof/disposal route may recover dirty/index/source facts only while
the runtime-created branch and checkout HEAD still equal the immutable
acquisition base, and current exact content matches its durable recovery guard.
`WorkflowWorkspaceSettled.candidate` means the exact proven terminal candidate;
it remains `None` for unresolved final state. The separate
`recovery_guard: Option<CandidateRecoveryGuard>` contains a `reference` copied
from native `CandidateScope.state.current`, the last successful candidate proof
before uncertainty. It is not a final candidate, execution authority, or
model-visible JSON: it is only a destructive-recovery comparison baseline.

If a writer's `finish(false)` failed after changing A to B, only A can guard
recovery, so B is preserved. If `finish(false)` proved B and final run inspection
later failed, the guard is B. Recovery may remove unchanged B, but later B-to-C
edits are preserved. Missing guard means no destructive authority.

An unresolved terminal fact has no trusted durable terminal HEAD: if the Agent
committed a newer HEAD before inspection failed, readable
current Git facts cannot authorize that commit's disposal. The worktree and
branch remain retained for explicit user/manual recovery. No borrower or
execution is recreated. Missing terminal settlement remains
conservatively `NestedContainment`.

```text
Retained -> DisposalStarted (durable exact intent)
         -> worktree removal -> branch/ref settlement
         -> DisposalSettled (durable physical phase)
```

The native physical owner (`WorkspaceManager::dispose_authorized_workspace_inner`)
recomputes `inspect_source` and compares its content digest to the final candidate
or recovery guard, then re-proves exact ownership immediately before the first
`git worktree remove --force`. This comparison applies whenever the worktree
remains present,
including retries with committed intent. If removal succeeded but the durable
settlement append failed, retry uses that same intent and exact absence of
both path and Git registration to continue branch/ref settlement without
hashing the deleted checkout. If the branch was also removed, retry commits
AlreadyDisposed. A partial branch failure can commit WorktreeRemoved or retry
from the still-authorized intent. Repository identity, deterministic allocation,
registration and compare-delete ref proofs remain mandatory. Absence without
intent, partial absence, replaced paths, unrelated refs and changed retained
source before the destructive frontier fail closed. There is one native
physical disposal state machine; the Workflow wrapper only carries durable
identity, candidate applicability and phase facts.

## Human questions and business Review (WF-04)

`Tool(ask_user)` is an ordinary explicitly selected native capability. It uses
exactly the same normalizer, schema, executor, Questionnaire requester, and typed
JSON result as an Agent invocation. It is foreground, sequential, approval-never.
Its existing `cancelled: true` result means explicit questionnaire decline; it is
successful business data, not execution cancellation. Actual cancellation,
deadlines, provider absence and control failures never create that answer.
The native context reborrow also rebinds the requester to the driver's leaf
cancellation/deadline scope. Both leaf and finite outer deadlines remain active.

[The executable human_review example](../examples/local-runtime/workspace/.agents/workflows/human_review.yaml)
shows static capability selection, literal arguments, typed results, Branch and
Review. Register it explicitly through the usual Workflow resource configuration.
Its input object is the complete plan being reviewed; answering the question does
not rewrite that plan. The answer is explicit immutable review context.

There are three distinct contracts:

| Contract | Meaning | Result |
| --- | --- | --- |
| Questionnaire | Supply missing business information | Existing submitted/declined answer contract |
| Review | Accept/reject one immutable business subject | `{accepted: boolean, feedback: string}` |
| Tool Approval | Permit the exact prepared invocation to start | Existing Allow once/Deny |

FullAccess affects only configured Tool Approval. It does not answer either
Questionnaire or Review, select capabilities, or change workspace authority.
No Review policy or model/provider interpretation is involved.

A Review node has `type: review`, a `subject`, and a `context` array. The subject
must reference an already committed value, including a validated block input:

```yaml
human_review:
  type: review
  subject:
    type: candidate
    value: {type: reference, path: [implement]}
  context:
    - {type: reference, path: [check]}
```

`type: plan` freezes the actual structured object by value. It does not certify
files named by strings inside that object. `type: candidate` requires the referenced
value's interpreter-owned CandidateReference; authored JSON cannot manufacture it.
The reference includes the owning run, monotonically changing version and native
content digest. The WF-03 source contract (dirty tracked bytes, admitted untracked
source, deletions, index/modes/symlinks, exclusions and unsupported-content rules)
continues unchanged. Equal HEAD with different source bytes is a different subject.
Context/check entries are bounded typed facts `{value, candidate}`. The optional
candidate is the runtime-owned exact reference, not a claim from authored JSON.
Plan subjects also preserve their own optional candidate applicability. These
identities are included in the Review specification, digest, durable requested
fact and UI disclosure. Identical check JSON tied to A and B has different Review
identity. All subject/context dependencies must agree on one exact candidate;
conflicts fail before publication. Plan Reviews with candidate-bound facts acquire
and retain the same native freeze used by candidate subjects.

### Ownership and linearization

| Frontier | Owner and rule |
| --- | --- |
| Subject selection | Workflow evaluates a committed explicit reference and bounded context. |
| Candidate freeze | Workspace plane borrows the exact expected CandidateReference, starts native mutation observation and holds its exclusive borrow. |
| Requested commit | InteractionCoordinator commits InteractionRequested before installing/publishing the actionable prompt through the existing route. |
| Response validation | Coordinator checks the live identity, kind, concrete instance and whole-subject digest. CandidateFreeze validates while retaining that native borrow. |
| Settled commit | Under coordinator terminal ownership, durable InteractionSettled precedes waiter release. Observable cancellation/deadline overrides a response. Source invalidation is a separate runtime outcome, never rejection. |
| Review local commit | Workflow validates native freeze settlement and commits immutable decision/feedback JSON without candidate provenance. Accepted candidate identity is retained separately in run-local Acceptance state. |
| Downstream admission | A candidate-consuming Tool/Agent combines explicit data applicability with Acceptance and acquires the exact expected native borrow **before** WorkflowNodeStarted. Tool/Agent receives that same borrow through execution and physical settlement. |

The borrow may be released between settled Review and later admission. A queued
writer can win that interval and publish B. In that case the next admission of A
fails before its node starts. There is no unlocked currentness-check followed by
later execution: acquisition, exact-reference validation and the effect's access
are one ownership transfer. Pure Branch/Return reads of Review decision/feedback
need no candidate authority. Ordinary candidate-derived data and explicit old
candidate references still require currentness; pure consumers hold their native
borrow through consumption. Parallel passes acceptance separately from input data
and does not hold a parent borrow while branches wait. A mutating Agent consumes A
at its start; its resulting B needs a new explicit Review to gain acceptance.

Both accepted and rejected Review JSON are immutable business records. Rejection
establishes no standing candidate authorization: after Reject A, a writer can
produce B and Branch/Return can still read the decision and feedback. An explicit
A-bound producer remains stale and cannot start a repair Agent against B. Accepted
A likewise cannot admit a candidate-dependent operation against B. Acceptance is
not encoded in `CommittedValue.candidate`; that field remains data provenance.

Native writer exclusion is not a sandbox against arbitrary external host processes.
WF-03's kernel observation and content checks detect interference at the native
freeze/settlement/admission boundaries. Review does not claim a new containment
mechanism. Invalidation fails closed without automatic reprompt/retry.

Questionnaire facts now carry ordinary ToolInvocationId; Workflow invocations use
concrete run/block/node/visit identity. Review carries that same concrete node
instance. The original conversation owns every InteractionRef and waiter. Child
Questionnaire/Approval retains its child coordinator and existing reliable root
route; no parent-owned copy, model request or canonical transcript relay is created.
The root Runtime Client remains the only answer surface. Queue order and focus
never determine settlement ownership.

Human waiting consumes the outer wall-clock deadline. Cancellation observable
before downstream admission prevents that work. No capable provider before
publication fails closed. Detach after publication preserves the live waiter;
resync shows the same request without another audit/request. Process death removes
live authority. Neither audit history nor SQLite reopen reconstructs waiters,
Workflow execution or acceptance grants.

### Disclosure and limits

Plans are complete inline structured objects, limited to 32768 serialized UTF-8
bytes. Context has at most eight entries and 8192 total serialized bytes. Concrete
instance serialization is bounded to 8192 bytes. Candidate inspection paths are
native-generated and at most 4096 UTF-8 bytes. Feedback is at most 2000 Unicode
scalar values; Accept has no feedback channel. Review uses the existing Workflow
value/depth and aggregate local-data bounds as well.

The unified overlay starts on Reject. Accept requires explicit navigation. Ctrl+F
edits bounded rejection feedback; Enter exits feedback editing without submitting.
PgUp/PgDn scrolls complete plan/context rows and reports the visible range.
Candidate content is explicitly **not inlined**: the trusted inspection path is
shown with its exact candidate identity, and no shortened diff is presented as
complete. Inspection is read-only in meaning and conveys no write authority.
Esc dismisses Review presentation; it does not answer it. Removing one pending
item leaves unrelated drafts and interactions intact.

WF-04 uses Runtime Client/TUI 21 and child IPC 19 for Review and required
Questionnaire invocation correlation. SQLite development schema 29 freezes the
changed audit payloads; older stores are refused, without migration. Event
envelope version remains 1 because framing has not changed. No durable Workflow
or pending-interaction tables are added.
