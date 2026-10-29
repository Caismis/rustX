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

The Web **Workflows** area authors the same native YAML files that
`check` and `explain` inspect. One Workflow is one file,
`.agents/workflows/<id>.yaml`, in exactly one scope: **User** (shown as
"Global", under the User configuration directory) or one registered
**Workspace**. Rust owns the file's semantics, validation, serialization,
revisions and commits; the browser owns canvas interaction only. There is no
second runtime, no database program definition, no browser YAML compiler, no
persisted React Flow JSON and no draft or publish lifecycle: the file is the only
durable definition.

### Complete native file document

`WorkflowFileDocument` owns the native file and canonical YAML serialization. Its
flattened `definition: WorkflowDefinition` keeps the existing executable YAML
fields (`description`, `timeout_ms`, `block`); optional `editor` holds typed
presentation. The compiler receives only `definition`. There is no second loader
for Web: runtime discovery, CLI inspection, source reads, candidate checks and
all source writes use this file contract.

The file revision is SHA-256 of **all source bytes**, including presentation;
rustX writes normalized canonical bytes. CAS and copy/delete preconditions use
this complete revision. Candidate identity and lost-response recognition use the
exact complete canonical candidate, never program equivalence. Hand-authored
formatting remains part of the observed source revision. `WorkflowProgram`'s
digest hashes only the executable definition, including model-visible schema
`description` annotations. Metadata-only edits change file revision and leave the
program digest, compiled graph, Tool contract and execution behavior unchanged.
Already admitted programs and Attempts stay frozen across either kind of edit.
Commit, publication, observed reread and runtime adoption remain separate facts.

`editor` accepts only these collections; arbitrary plugin data is rejected:

| Collection | Authored content and identity |
| --- | --- |
| `nodes` | Structured `{block, node}` address; optional display `title`, author `description`, `position`, Loop/Parallel container `dimensions` |
| `entries` | Synthetic Block-input/entry-anchor `position`, addressed by its owning `block` |
| `branches` | Parallel owner address, branch key, lane position and dimensions |
| `notes` | Globally unique stable note ID, owning Block, Markdown text, one of `neutral/blue/green/amber/rose`, position and dimensions |
| `orders` | Ordered field names at a typed Block-input/output, node-output, node-input binding or Parallel-branch-list target |

A Block address is an array of `{node}` (Loop body) or `{node, branch}`
(Parallel branch). Explicit null branches are rejected. A Parallel branch named
`body` is distinct from a Loop body. Different child Blocks may reuse local node
IDs; executable nodes, entry anchors and notes have separate identity domains.
Schema order paths use `{kind: "property", name}` and `{kind: "items"}` steps,
so array-item property lists and properties literally named `items` are distinct.
Ordering affects display only; newly declared fields follow in deterministic key
order, independent of map insertion order.

For example, an executable file can include:

```yaml
editor:
  nodes:
    - address: {block: [], node: reviews}
      title: Review lanes
      position: {x: 200, y: 80}
      dimensions: {width: 900, height: 600}
  notes:
    - id: review_note
      block: [{node: reviews, branch: quality}]
      text: "## Review guidance\n\nKeep the result concise."
      theme: blue
      position: {x: 0, y: 100}
      dimensions: {width: 240, height: 180}
  orders:
    - target: {kind: branches, address: {block: [], node: reviews}}
      fields: [security, quality]
```

Missing metadata is ordinary valid input. The current canvas derives the same
deterministic default positions every time and writes an authored position only
when the author moves/adds a node. Renderer measurements, React Flow graphs,
callbacks, selection, results, credentials, cached catalogs and analysis replies
are never file metadata. See the [visual authoring guide and acceptance record](workflow-visual-editor.md) for the delivered interaction and ownership contract.

### File and presentation limits and repair

| Boundary | Limit |
| --- | --- |
| Complete serialized file | 1 MiB, also enforced by the common finite source reader |
| Expanded authoring document in JSON | 768 KiB, leaving space in the existing 1 MiB transport envelope |
| Executable definition | 512 KiB; existing 256-node, depth-8 Block and value/run budgets remain separate |
| Editor metadata | 256 KiB and 1024 total collection entries; at most 128 notes |
| Node title / author description / note Markdown | 256 / 4096 / 16384 UTF-8 bytes per entry |
| Coordinates / dimensions | finite x/y in ±1000000; finite width/height in 16..8192 |
| Addresses / field order | Block depth 8; schema path depth 32; at most 256 unique ordered fields, each 1..64 UTF-8 bytes |
| Presentation diagnostics | At most 64; static reasons and bounded collection-index paths |

Malformed/oversized metadata, duplicate identities and non-finite geometry are
presentation failures and refuse admission/serialization without replacing the
file. File-size and YAML-syntax failures remain genuine file failures. Unknown
executable fields remain executable-language rejections. Presentation bytes are
never charged to runtime node counts, retained values or executable bytes.

Orphaned addresses and kind changes are **retained with presentation-specific
warnings**, not erased, automatically retargeted or treated as executable type
failures. Reads keep the complete editable document; Check returns `presentation`
warnings separately from structure/dependency facts. The native CLI captures
warnings with the same source snapshot and reports them separately from the
compiled program. Repair/remove the indicated entry explicitly, or restore its
target. Bounded orphaned entries may be saved/copied intact, allowing legitimate
external executable edits without requiring coordinated metadata surgery.

Node and branch rename commands move the addressed metadata and private Block
subtree atomically with executable references. Deletion removes only the deleted
node/subtree's metadata: a note owned by a still-existing parent Block remains.
Node copy clones its owned presentation and assigns deterministic fresh note IDs;
file copy carries the exact selected saved source revision's complete document.
A rename/copy that collides with an authored orphan entry is refused visibly,
never resolved by discarding authored content. The original destination identity
and its private Block subtree are checked before the edit, even when the source
has no presentation entry to move or copy. This includes a branch rename
that would duplicate a retained field inside either branch order or root
Parallel-output order. The entire executable/metadata transaction is rejected.

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
identity.

The native App Server is a separate constraint. A port also captures the
native authority identity (the `authority_id` of the `initialize` the browser
already observed) and every Host-routed Workflow operation carries it as
`native`: Workspace list, read, analysis, check and write, and a Global file's dependency
check routed through a Workspace. The Host opens its own connection per
operation; after initializing it and before dispatching any Workflow request,
it compares the server's identity with `native` and refuses a mismatch
(`native_replaced`). Another App Server process at the same endpoint therefore
never receives a transaction bound to the one it replaced, even when a missing
target or an unchanged revision would pass its revision check. A missing
expectation is refused, never filled in from the newly reached server, the
latest browser snapshot or a later listing. Such a refusal is a known,
unavailable outcome with no Workflow request sent, never an uncertain write.
Reconnecting to the same native authority keeps working: a transport
generation is not an identity. An acknowledgement already received stays a
committed fact; a later authority change only leaves its reread unconfirmed,
and nothing is replayed.

An open editor, a pending library operation and an open confirmation
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
in it. An uncertain outcome retains the dispatched transaction (confirmation
sequence, port, operation and expected revision) and permanently disables
submission of that confirmation, even while its next listing is pending, failed
or complete. Refresh observes only: a file appearing or disappearing does not
prove that this transaction committed. The author can close the confirmation,
inspect the current file and explicitly start a new operation; the old operation
is never replayed.

IDs follow the native `WorkflowId` rule. Paths are resolved by native code
relative to the scope's admitted directory handle: traversal, absolute paths,
symlinked directories or files and a scope redirected after admission are
rejected, and a Workflow directory holds at most 128 files (see Capacity).

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

### Candidate output catalogs and variable picking

`workflow/analyze` accepts `{target, id, document, generation, query?}`. It performs
no dependency discovery/preparation and constructs no `WorkflowProgram`. A
candidate with an unfinished instruction, binding or graph can expose its output
contracts before it could pass Save/Check. `outputs` distinguish declared/derived
contracts from incomplete contracts (`schema: null` with a static diagnostic).
Branch and Return are not business-data producers; neither are entry markers or
notes. Text Model output is a string, with no invented `.text` field.

The file definition retains editable Agent profile, Model/Model Profile and Tool
source/name spellings, including an unselected empty string. Analysis does not
resolve these selections. Compilation lowers them through the existing native
identity constructors and rejects invalid selections before admission; compiled
programs retain the native identity types. Saving still requires compilation.

An optional query names one exact consumer: structured node address, typed field
(Model/Agent named input, Tool arguments, Review subject/context, Branch
condition, Return output, Parallel branch input, Loop initial input/`until`/`carry`)
and structured steps within a construction or predicate. It can search paths or all matching qualified producer titles,
limit results and classify one existing/manual `reference`. It does not construct
a Cartesian product of all fields and consumers.

The compiler and analysis share control-flow proof, node-entry scope intersection,
reference resolution, Model-output validation, aggregate output construction and
schema compatibility. An unfinished binding does not require an alternate Rust
compiler or a browser reachability/type engine. The response reports separately:

- lexical availability: `available`, `unavailable`, or `pending`;
- structural compatibility: `compatible`, `incompatible`, or `unknown`;
- declarations, bounded diagnostics, `incomplete` and truthful `truncated` facts.

Availability is a lexical fact of this exact candidate, not proof that a producer
ran or that dependencies/admission are ready. If the candidate's control flow
cannot be proven, known declarations remain visible and availability is pending.
Producer presence and output-schema knowledge are independent facts. An
unfinished output schema still contributes its producer to the native scope
intersection. A guaranteed upstream producer with that schema remains pending;
self references, forward references and producers bypassed by a join remain
unavailable even before their output declarations are completed.
An unfinished Block input contract likewise marks only `args` as having an
unknown schema; valid control flow still proves producer presence and absence.
The entry environment and join intersection do not require input type knowledge.
Within one Block, a producer has one identity and a fixed declaration. Both
compilation and analysis retain that identity at a join exactly when every
incoming scope contains it. They do not test the declaration against itself:
finite `enum` candidates can be filtered by other constraints without changing
producer identity. Consumer binding checks separately establish assignability
to the destination schema; presence alone never proves type compatibility.
Unsupported/optional field traversal remains invalid. Unknown external Tool
argument contracts remain unknown; no MCP/Python preparation occurs. Whole-object
and whole-array references are supported; array indexing and optional fallbacks
are not introduced. The picker may preserve a pending reference in memory, while
Save/Check keep their existing authoritative compiler rules.

Tool argument roots share the compiler's necessary object requirement: a known
string, array or other non-object root is incompatible. An object remains unknown
until the complete Tool contract is known; scalar values inside an argument
object do not inherit its root requirement. Native aggregate output schemas are
retained as derived, including the extra wrapper around a maximum-depth authored
Parallel/Loop output. Only the explicit unknown-schema marker lacks a type proof.
Equality and inequality share the compiler's boolean/string/null operand rule.
Known numeric, object and array operands are incompatible even when the other
type is unknown. Known supported operands must have matching types; otherwise
missing type facts remain unknown. Constructor root types remain known while
their fields/items are unfinished. These necessities do not add numeric predicates.

Destination contracts are validated before projecting into constructed fields.
An undeclared property of an object with `additionalProperties: false` is
incompatible for every reference, even when its producer type is unknown. This
prohibition persists through descendant fields and array items, with an explicit
diagnostic. Return output, Parallel branch input and Loop input/carry use the
same native rule. Undeclared fields of open objects and unknown external Tool
contracts retain unknown compatibility; declared properties use their schemas.

Array item choices also share the compiler's common element-contract inference.
The exact selected value is replaced in each enclosing array construction for
the proof; sibling contracts are retained, including through nested fields and
arrays. A candidate must satisfy the destination and the construction itself.
A wider actual element contract may admit other elements, and native finite
const alternatives remain valid. Unknown sibling schemas keep construction
compatibility unknown instead of claiming compatibility from `items` alone.
References borrow their declared schema; unchanged siblings retain shared
handles, so repeated references do not retain repeated schema trees. The same
native inference serves compilation and authoring, with authoring additionally
charging work before traversing, hashing, comparing or serializing, and charging
materialization before cloning or expanding derived objects/arrays. Its per-query
limits are 4 MiB of cumulative weighted allocation, 32 MiB of schema work and
32768 inference/measurement steps, independently of the requested result count.
Weights conservatively include tree/container overhead and escaped text; they
are accounting units, not a process RSS limit. Exhaustion marks the response
`truncated` and `incomplete`, leaves unfinished compatibility proofs `unknown`
with a budget diagnostic, and retains established availability and necessary
type restrictions. It never turns incomplete work into a compatibility proof.
Reference lookup caches successful and invalid paths. Native compatibility
meters each recursive proof, reserves validator storage before construction and
charges nested finite-value validation against the complete destination schema.
Required-property chains recurse once per property, and declaration validation
checks enum/required uniqueness with borrowed sets. Equality reads the resolved
type without measuring or copying the surrounding schema.

The output inspector distinguishes a current analysis request in progress, a
failed request, an output omitted by response truncation, an incomplete output
declaration and a ready declared/derived schema. It never renders a failed or
truncated request as a reason to keep waiting for an incomplete declaration.

Ordinary fields use node-entry scope. Parallel input mappings and Loop initial
input use the parent environment. Private child nodes see only their projected
`args` and their own producers. Loop `until`/`carry` see only committed body-output
`result`; downstream parent nodes see only Parallel/Loop aggregates. Joins use the
intersection of required predecessors, never an ancestor union.

Requests retain executable limits and additionally cap a query at 16 KiB, search
at 256 UTF-8 bytes, expression/reference depth at 32 and the opaque generation at
64 bytes. A query requests 1..256 results (the current picker requests 128).
Traversal visits at most 2048 field positions, with a bounded pending frontier.
Core analysis is bounded to 256 KiB; the complete response, including query,
source authority and presentation diagnostics, is bounded to 512 KiB. Search can
narrow a truncated result; truncation never establishes a proof.

Replies echo native target/ID, complete canonical candidate revision, browser
edit generation and exact consumer/query. Native candidate identity, edit
generation and connection/resource authority are separate. The Product Host
checks the target against its registered canonical Workspace. The browser fences
responses by port authority, target, immutable document identity, generation,
consumer, search and request sequence, and refuses mismatched echoes. It hides a
cached result synchronously when any owner changes, before effect cleanup. Older
completions cannot validate a new generation or populate another inspector.

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

### Capacity

Discovery has two bounds. A resource directory holds at most 128 `.yaml`
entries, and one composed catalog (User alone, or User with one Workspace after
shadowing) at most 64 distinct candidate Workflow IDs, counted from valid
filename identities before any file is compiled, so a file with invalid content
still occupies its ID. Discovery also reads at most 1024 authored entries of any
kind in a directory, besides rustX's one `.rustx-locks/` lock directory, which it
reads past without counting (the scan is capped at 1026 raw entries). Past any
bound, discovery reports the directory or catalog instead
of the files in it.

`create` and `copy` add a file, so native code admits them against these bounds
before publication and refuses an addition that would pass one with
`workflow_invalid`, category `capacity`, and a correction; the destination stays
absent and existing discovery is unchanged. The scopes counted are exactly the
ones the operation is authorized for:

- a Global addition counts the User directory and the User-only catalog;
- a Workspace addition counts that Workspace directory and its composition with
  User (an override of an existing Global ID adds no identity);
- the directory added to must keep room for the published file within the
  1024-entry bound, counting the staged candidate that briefly exists beside it
  during publication.

rustX's own writer metadata never consumes these bounds: the persistent writer
locks of a directory's documents, and its capacity lock, all live in that
directory's single private `.rustx-locks/` directory, which discovery never
counts or reads. A first write into a directory an external tool filled to the
bound creates that directory without changing what discovery admits: a refused
`create` or `copy` leaves the catalog readable, and an acknowledged `replace`
leaves the saved file listed and readable, in every scope the write locked
(including User, whose capacity lock a Workspace addition takes).
Create/delete cycles and refused additions with ever new IDs leave locks only
there, so neither a successful nor a refused addition can push existing files
out of discovery. A `replace` stages its candidate beside the file, so while it
publishes into a directory already at 1024 authored entries a concurrent
discovery read can briefly see one entry too many; the saved directory is back
within the bound when the write returns. Arbitrary files placed in a
Workflow directory by other tools still count, and the bounds still refuse an
addition they leave no room for.

A Global addition cannot see the Workspaces a later session may compose with,
so it does not prove that every Workspace composition stays within 64 IDs; a
Workspace addition is the one that checks its own composition.

Count-changing rustX writes take a persistent advisory capacity lock per
physical Workflow directory before the file's own writer lock, always User
first, then the Workspace. A Workspace whose Workflow directory is the User one
(a Workspace at `~/rustx`) is recognized by the directory's device and inode,
and its addition takes that one lock once; its logical scope, authorization and
counting are unchanged. Two
additions competing for the last slot, in one process or several, are
serialized and at most one succeeds,
and a Workspace addition waits for, and counts, a concurrent Global addition.
The file revision check, directory-handle protections and atomic publication
are unchanged. Like the writer lock, the capacity lock coordinates rustX
writers only, not editors or Git. `replace` and `delete` never add an
identity: they keep their revision checks and stay available to bring an
externally over-full directory or composition back within bounds.

### Concurrency guarantee

Writers inside rustX are serialized by the source's advisory writer lock, and the
expected revision is checked under that lock immediately before the rename, so
two rustX writers can never both commit against the same base. Writers outside
rustX (an editor, `git checkout`) do not take the lock; their changes are
detected by revision comparison on the next read or write, but a change that
lands between the final check and the rename is not excluded. Writes are atomic
renames, so a file is never observed half-written.

### Editor state

Authored node positions are stored only in the YAML `editor` metadata. Completed
node drags and node additions change the complete working document, mark it dirty
and enter its undo history. Save commits executable and presentation changes in
one CAS transaction. **Organize layout** lazily runs ELK, placing child Blocks
before their containers with measured dimensions and stable control ports. It
changes metadata in one undo step and preserves manual notes. A result from an
older document, target, layout request or drag is discarded. Personal viewport
pan/zoom belongs to the exact file; inspector width is another bounded personal
preference. Neither enters file content. The obsolete per-Block layout cache and
reset-to-default engine are removed without migrations or parallel layout paths.

Unsaved edits live in memory: leaving a dirty editor asks for confirmation, and
there are no persisted drafts.

Structure checks belong to the document generation. Global dependency results
also belong to the Workspace used by that check: selecting another Workspace
hides the previous dependency result and its canvas warnings and shows **Not
checked** until that context is checked. The independent structure result stays
visible.

Every editable field has an editor-internal address (`FieldAddress`): its lexical
block path, its owning node (none for a block's own contract), and its path as
separate components, with authored object keys (`{ key }`) kept distinct from
structural properties and array indices. Its canonical key is the JSON encoding
of that tuple, so a direct field `a.fields.b` and the nested field `a → b` are
two fields, and every key the native format allows stays editable. One address
owns a field's unapplied text, its error, its commit registration and its edit
session. A typed domain distinguishes executable fields from node presentation
and note fields; optional numeric fields and pending new keys have explicit
roles. Human-readable labels and native diagnostic paths are separate from it.

The editor machine owns every piece of unapplied field text (a JSON schema or
literal being typed, bounded title/description/note text, incomplete numbers,
node/branch/schema-field renames or a new mapping key) as a
versioned draft under its address. A draft is dirty for leaving and unloading
and survives selection, container focus, disclosure changes and component remounts or
reuse; only a successful application, an explicit discard, or a confirmed reload
clears it. Blur, Enter, Save and Check request an application (`APPLY_DRAFT`)
of one exact draft version. In one context update the editor runs the field's
command against the current document and decides the outcome. Node-specific
form commands also validate the expected node kind at execution time: a retained
Agent callback cannot modify a Tool that later reused its ID. Refusal leaves the
document and history unchanged. The draft outcomes are:

- `applied`: the document and history change and exactly that version is
  cleared; newer text of the same field is never cleared;
- `unchanged`: the field already holds that value, and the draft is cleared;
- `rejected`: invalid JSON, a number outside the contract, an empty key, or a
  refused structured command (for example `duplicate_identifier`) keeps the text
  with its error and changes nothing.

A renamed node, branch or schema property takes the drafts of its owned fields
along to their new addresses. Rename Undo/Redo carries attached drafts back and
forward through the accepted identity change. When another change (undo or redo, for instance) changes a field that
holds a draft, the text is kept and marked as changed: it is neither dropped nor
reapplied, until the author keeps it over the current value or uses the current
value. Changes to other fields do not affect a draft. Attachment and conflict are
independent: a draft marked as changed still belongs to its field, follows it
through renames and array edits like any other attached draft, keeps its mark
while doing so, and is detached when its field goes.

When the field is gone (its node or branch deleted, its value kind changed, or
its array item removed), the draft is detached: it keeps its text and its
original location for the pending-field list, under an identity of its own that
no field ever shows. The pending-field list (shown when Save or Check is held
back) offers **Show kept text** for it: a read-only field with the exact text,
whitespace and line breaks included, labelled with the original location, and a
**Copy** action. Showing or copying applies nothing, changes no draft and sends
nothing; when the clipboard refuses, the text stays shown for manual selection.
A detached draft is never reattached automatically; the author recovers its text
this way and then discards it, and its draft keeps the editor dirty and blocks
Save and Check until then. It never follows a
rename and never occupies the address another draft moves to, so a node or
branch renamed to the name of a deleted one migrates its own drafts and shows
its own text; the deleted one's text stays listed apart, and a field created
later at that place never takes it over. A rename that would still place two
drafts under one address is refused, with its text kept and nothing changed, and
no other structural change ever lets one draft replace another: a draft whose
destination is already taken is detached instead.

Array items have no identity in the native format, so the editor tracks it per
edit instead. Removing or appending an item through the inspector is one known
array edit: drafts of items before the removed one keep their index, drafts of
items after it move down by one, and the removed item's draft is detached; an
append moves nothing. Undo and redo apply the inverse or the same edit to the
drafts. A change of an array's length that is not such an edit leaves item
identity unknown, so drafts inside that array are detached rather than guessed;
identity is never inferred from equal values. This metadata is editor state
only and never reaches the file.

Save and Check, by button or by Ctrl/Cmd+S from a focused field, first request the
application of every shown field, renames last, and then pass the editor's
pending-field gate. While any draft remains (invalid, rejected, conflicting, or in
a field not shown), nothing is sent, and the notice lists each such field with
Show (it expands the owning container and disclosure, then focuses the exact
field) and Discard. Copy and Duplicate refuse to export an owned subtree with
unapplied input; resolving or discarding the listed input makes the operation
available.

Undo and redo share one bounded timeline of complete-document edits. One
completed node drag is one metadata edit: undo restores the previous complete
document before any older edit. Pointer movement stays transient; only the
completed drag changes the candidate and its generation. Nothing is written until
Save.
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
