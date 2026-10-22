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

A transaction is bound to the authorities it started under, not only to IDs.
Each browser port captures the App Server endpoint and authority revision and
the Product Host authority (`WorkspaceAuthorityScope`) current when it is
created. Every Workspace operation, and a Global file's dependency check routed
through a Workspace, carries that Product Host scope; the Host validates it in
the registration lane immediately before native dispatch. A replaced Host is
refused (`authority_replaced`) even when it reuses the registration ID with a
different directory, so revision equality never stands in for resource
identity. An open editor, a pending library operation and an open confirmation
dialog each keep the port they started with; refreshing the library creates a
new read port for the listing only, and is never applied to them.

### Library observations and confirmations

The library machine accepts each listing together with the exact port that read
it, as one immutable observation (`LibraryObservation`). A refresh reads on a
new port while the previous observation stays visible, and a read superseded by
a newer one has no completion path. While a replacement read is pending, no new
Open, New, Copy or Delete starts: the buttons are disabled and the machine
ignores such requests, so a listing of one read is never combined with the port
of another.

A confirmation is machine state. It captures its observation when it opens: the
scope, directory and listing it shows, and for Copy and Delete the exact source
file and revision. The dialog submits only the author's values; the machine
builds the operation from that observation, so what a confirmation shows is
exactly what it changes. A confirmation opened before a refresh stays bound to
its observation; submitted during the refresh it is accepted and runs on its
own port, which refuses (`product_host_replaced`) if its authority was replaced
meanwhile, never redirecting to the replacement. A dispatched operation keeps
its port, expected revision and result path: a refresh waits for its outcome,
and a scope chosen meanwhile is selected after it. The confirmation closes only
when that operation succeeded; a failure, including an uncertain outcome, stays
in it. An uncertain write is observed by the next listing, never replayed.

IDs follow the native `WorkflowId` rule. Paths are resolved by native code
relative to the scope's admitted directory handle: traversal, absolute paths,
symlinked directories or files and a scope redirected after admission are
rejected, and a Workflow directory holds at most 128 files.

### Reading and invalid files

`workflow/list` returns one bounded summary per file (ID, relative path, exact
revision, byte length, modification time, a description bounded to 512 bytes,
node count and an optional diagnostic). A malformed file is still listed with its
bounded diagnostic and stays openable read-only; it is never hidden or repaired.
Each serialized entry is held to a fixed 7 KiB share, so the at most 128 files of
a scope always list within the 1 MiB message bound. Only an entry authored to the
extreme is reduced, dropping first the compiler detail, then the diagnostic path,
then the description; its identity, revision and diagnostic category remain, and
no entry ever displaces another.
`workflow/read` returns the strictly parsed document, its revision and the
vocabulary the editor offers (named Agent profiles and selectable native Tools).
Diagnostics carry the native authored path (`block.nodes.<id>.…`), a category,
static reason and correction text, and parser coordinates; authored values and
parser messages are not copied into them. The path is bounded to 512 UTF-8 bytes
(a longer one keeps its longest whole-component prefix, so it still locates the
owning block) and the compiler detail to 1 KiB.

### Number contract

Workflow documents cross the App Server as JSON. A browser reads every JSON number
as one binary64 and writes it back as the shortest decimal of that binary64, so
the editable domain is exactly the numbers whose browser round trip returns the
same native number:

- integers written without a fraction or exponent, within ±(2^53 − 1);
- non-integral finite floats, and integral floats from 1e21 (which a browser
  prints in exponent form, so they stay floats).

Everything else is outside the editable domain, so numeric editing is not
lossless in general. An integer beyond that range would be rounded. An integral
float below 1e21 (`1.0`, `1e3`, `2.50e1`) would come back as an integer, which
native types differently: a constructed array of `1.0` and `1.5` would then no
longer compile. Negative zero in any spelling (`-0`, `-0.0`, `-0e0`) would be
written as `0`. Native applies this one rule (`unrepresentable_number`) on read
and to every candidate:

- a file holding such a number is listed and opened read-only with category
  `unsupported_number`, located at the number's authored path, and its bytes are
  untouched;
- `workflow/check` and a `replace` write refuse a candidate holding one.

The browser judges authored text before conversion, from each number's source
token (`JSON.parse` source text access), at any depth of the typed value.
Negative zero is refused before the integer rule, since `-0` is spelled like an
integer. A field holding such a number keeps its text with the error and is not
applied, so no rounded, retyped or re-signed value is ever sent.

### Checking

`workflow/check` validates a candidate document without writing it. The report
keeps two facts apart:

- **Structure** — the result of `WorkflowProgram::compile` (graph, lexical scope,
  schemas, bindings, bounds): `valid` with every node's output contract and the
  `revision` a `replace` with this candidate commits as (the revision of its
  canonical serialization), or `invalid` with one bounded diagnostic at its
  authored path.
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
**uncertain**, not failed. Reconciliation only observes; it never resends the
write. The editor rereads the file: an unchanged revision means the write did not
commit. Otherwise it asks native for the attempted candidate's identity (the
`revision` of its check) and compares that with the reread revision. Because the
identity is native's own canonical serialization, a commit that native normalized
(an omitted timeout written as the default, an ordinary edge's port) is still
recognized as the editor's own, while a meaningful difference — an explicitly
empty override against an omitted one — is a different identity. A recognized
commit becomes the new base revision; any other revision is reported as changed
elsewhere. Edits made after the attempted save are kept, unchanged and dirty, in
either case.

A received acknowledgement is a fact about the original target. If the App Server
or Product Host authority changes while the commit is being reread, the commit is
kept with an unobserved reread (committed and unconfirmed); it is never turned
into an uncertain or failed write, and the editor is never rebound to the
replacement.

### Concurrency guarantee

Writers inside rustX are serialized by the source's advisory writer lock, and the
expected revision is checked under that lock immediately before the rename, so
two rustX writers can never both commit against the same base. Writers outside
rustX (an editor, `git checkout`) do not take the lock; their changes are
detected by revision comparison on the next read or write, but a change that
lands between the final check and the rename is not excluded. Writes are atomic
renames, so a file is never observed half-written.

### Editor state

Node positions and the viewport are presentation only. They are kept in browser
`localStorage`, keyed by server, scope, Workflow ID and block path, and are never
written to the file; clearing them only resets the layout.

Unsaved edits live in memory: leaving a dirty editor asks for confirmation, and
there are no persisted drafts.

Every editable field has an editor-internal address (`FieldAddress`): its lexical
block path, its owning node (none for a block's own contract), and its path as
separate components, with authored object keys (`{ key }`) kept distinct from
structural properties and array indices. Its canonical key is the JSON encoding
of that tuple, so a direct field `a.fields.b` and the nested field `a → b` are
two fields, and every key the native format allows stays editable. One address
owns a field's unapplied text, its error, its commit registration and its edit
session. Human-readable labels and native diagnostic paths are separate from it.

The editor machine owns every piece of unapplied field text (a JSON schema or
literal being typed, a node or branch rename, a path component being added) as a
versioned draft under its address. A draft is dirty for leaving and unloading
and survives selection, block navigation, tab changes and component remounts or
reuse; only a successful application, an explicit discard, or a confirmed reload
clears it. Blur, Enter, Save and Check request an application (`APPLY_DRAFT`)
of one exact draft version. In one context update the editor runs the field's
command against the current document and decides the outcome:

- `applied`: the document and history change and exactly that version is
  cleared; newer text of the same field is never cleared;
- `unchanged`: the field already holds that value, and the draft is cleared;
- `rejected`: invalid JSON, a number outside the contract, an empty key, or a
  refused structured command (for example `duplicate_identifier`) keeps the text
  with its error and changes nothing.

A renamed node or branch takes the drafts of its other fields along to their new
addresses. When another change (undo or redo, for instance) changes a field that
holds a draft, the text is kept and marked as changed: it is neither dropped nor
reapplied, until the author keeps it over the current value or uses the current
value. When the field is gone (its node or branch deleted, its value kind
changed, or an earlier array item removed so indices shifted), the draft is
marked as belonging to a field that no longer exists; that mark is permanent, so
an old address never takes on the meaning of a field created later. Changes to
other fields do not affect a draft.

Save and Check, by button or by Ctrl/Cmd+S from a focused field, first request the
application of every shown field, renames last, and then pass the editor's
pending-field gate. While any draft remains (invalid, rejected, conflicting, or in
a field not shown), nothing is sent, and the notice lists each such field with
Show (it opens the node and tab that hold it) and Discard.

Undo and redo share one bounded timeline of semantic edits and view steps. One
completed node drag is one view step: undo restores the dragged nodes' previous
positions, in their own block, before any older semantic edit. Pointer movement
never validates or writes the file, and view steps never become file content.
Typing in one field coalesces into one step; the session ends at a selection
change, block navigation, Save, Check, a node move, and undo or redo, and is keyed
by the field's full block, node and field identity.

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
