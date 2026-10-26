# Scheduler S0: native writer ownership verification

Date: 2026-10-22. Source baseline: rustX
[`df06c3071398aed6d795e8837beb002773dae668`](https://github.com/Caismis/rustX/commit/df06c3071398aed6d795e8837beb002773dae668).
This is the source-level verification for the
[Scheduler implementation plan](issue-85-scheduler-implementation-plan.md).
No StorageOnly runtime code is implemented by this document.

## Result

The existing manager can own finite cold writes without a second writer registry
or a resident Agent per cold node. The design requires changes to all registry
consumers below; adding only an Entry variant is insufficient. Keep one closed
schedule-storage interface inside SessionRuntimeManager. Callers supply a typed
command and receive owned facts, never a store, runtime clone or allocation lease.

The source audit establishes an implementation route for S1. It does not establish
the safety of an implementation that does not yet exist. The new interleaving
tests below are required before the first slice is accepted. S4a's executable
resource reconstruction and the scan performance gate remain separate work.

## Existing ownership and concrete changes

| Source / current behavior | StorageOnly implication |
| --- | --- |
| [Manager types](../src/local_runtime/session_runtime_manager.rs): Entry is Loading, Loaded or Unloading; by_session covers every current entry. | Add a finite StorageOnly entry carrying SessionId and exact node/conversation identity. Keep by_session as the index for runtime residency only; enumerate storage ownership from entries. |
| RegistryState::check_session rejects any other resident node; fenced also derives Session identity through by_session. | Split the predicates: retirement applies to every storage/runtime entry; runtime-node exclusivity applies only to load/replace. Cold mutation of A must not call the predicate that rejects resident B. |
| RegistryState::reserve counts entries.len(); diagnostics enumerate by_session. | Count runtime variants for runtime capacity. Bound storage flights independently with a fixed internal cap (initially 64), and report storage/retirement state separately from resident Agent counts. A full runtime quota alone must not reject cold writes. |
| [load/replace](../src/local_runtime/session_runtime_manager/lifecycle.rs) register a flight before acquire_session; waiters receive watch results and re-resolve. | Match StorageOnly in both paths, wait for proven writer release, then recheck retirement and runtime-node exclusivity. Never overwrite its slot with Loading. |
| [ManagedRuntimeClient](../src/local_runtime/session_runtime_manager/client.rs): admit_operation counts an OperationLease under the same mutex as Loaded -> Unloading; start_operation owns the lease in a task. | Hot schedule commands use this seam. Do not expose ManagedRuntime's weak resident as a source of strong runtime/store clones. Unload must see and await the operation. |
| spawn_unload waits for operations, native physical settlement and projection drain before taking composition; WriterCertainty distinguishes absent from unproven. | Reuse the certainty protocol for cold storage completion. Committed command outcome and successful writer retirement are separate facts. |
| TerminalGuard::finish activates a successful runtime candidate before its final fence/publication check. | A StorageOnly success must not run that activation arm. Publish a storage receipt and release evidence without calling compose, activate, model bindings or display-preview composition. |
| [delete_session](../src/local_runtime/session_runtime_manager/sessions.rs) captures one by_session resident; recover_session_deletion checks that same index for absence. | Under the retirement fence capture every entry belonging to the Session, including StorageOnly nodes. Recovery must reject an unresolved storage owner even when by_session is empty. |
| [drain](../src/local_runtime/session_runtime_manager/drain.rs) snapshots entries, calls unload, and includes entries in emptiness/failure diagnostics. Idle reaping selects Loaded only. | Extend retirement dispatch and diagnostics for storage; keep storage out of runtime idle reaping. A retained failed storage slot prevents successful quiescence. |
| [Configuration observation](../src/local_runtime/session_runtime_manager/configuration.rs) uses by_session and requires Loaded to select an application/runtime. | Preserve runtime-only lookup: StorageOnly A cannot replace B's configuration source, create an application runtime or make cold management appear resident. Model selection keeps this lookup and its existing source/Attempt fencing. |
| [Host drain](../src/app_server/host.rs) closes external admission, drains once alongside requests, then drains a final inventory. The manager itself has no host gate. | Wire Scheduler admission closure and driver/activation settlement into this host sequence. Internal timer work must not survive merely because it was not an RPC request. |

`RegistryState::remove(id)` currently removes that conversation's entry and only
matching by_session values. Preserve that property: retiring StorageOnly A must
not remove B's runtime claim. A cold entry must retain its own Session identity
until release is proven, including on cancellation/panic and failed close.
The registry consumer inventory covers the manager root and its client, lifecycle,
sessions, drain and configuration modules; model_selection has no independent
Entry/by_session state machine to extend.

## Storage access seam

[`SessionController::acquire_session`](../src/local_runtime/session_controller.rs)
and [SessionCatalog::acquire_session](../src/local_runtime/session.rs) resolve an
exact node and retain a shared ConversationAccess. That retention excludes
destructive allocation removal, but does not itself serialize writers.
[`ProductController`](../src/runtime/local_storage.rs) excludes another product
writer process; the manager serializes writers within this process.

The [SQLite open implementation](../src/durable/sqlite/schema.rs) supplies three
different operations which must not be conflated:

- open may create/initialize development state; composition and private Session
  preparation use that capability. It is not the cold management entry point.
- open_existing is READ_ONLY and validates the file, schema and bound identity.
  Discovery uses it; it does not authorize writes.
- recover_existing is an existing-only READ_WRITE open under ProductController
  and retained allocation authority. Its validation and writer configuration are
  reusable for a manager-owned storage flight, but its caller still needs the
  manager reservation and commit fencing.

Factor an existing-only writer-open helper accepting the retained native access
and private manager reservation. Do not pass arbitrary file paths or return its
store to Scheduler. The controller's existing recover_session_storage traverses
the Session graph and repairs projections without consulting manager residency;
do not invoke it as a shortcut for a cold command. Explicit recovery of a managed
conversation must join the same reservation, and must not bypass an unproven slot.
Private creation/child preparation and offline/read-only inspection remain their
existing owners' responsibilities; Scheduler gains no route into them.

## Finite operation and transfer protocol

Use a private `ScheduleStorageCommand -> ScheduleStorageResult` seam. The closed
commands cover management and due admission; a backend callback/SQL connection is
not part of the caller's interface. Each cold flight executes one finite command.
Another command waits for release and retries selection; it cannot indefinitely
append work to the open flight and starve a waiting load or delete.

1. Resolve exact Session/node identity without allocation ownership. Under the
   registry, refuse retirement/drain; select the exact hot incarnation or reserve
   StorageOnly before touching the cold allocation. Release the registry mutex.
2. For a hot command, acquire its OperationLease atomically with the Loaded check.
   For a cold command, a manager-owned task obtains SessionAccess and opens the
   existing writer. Revalidate identity and tombstone state after acquisition.
   The caller only retains a result receiver; abandoning it does not abandon cleanup.
3. After preparation, obtain product ownership-mutation participation outside
   registry/lifecycle/SQLite mutexes, then obtain the final counted Schedule commit
   permit under the same registry protection as retirement closure. Recheck command cancellation,
   request authority, Session/node and process eligibility. No permit is retained
   while awaiting another flight, provider or capacity.
4. With that participation and permit held, enter the hot lifecycle/publication
   seam or cold commit seam in the ordered phases below. Commit the native
   schedule/target/receipt transaction; then publish bounded owned facts. A permit
   acquired before a retirement request may finish; retirement is established
   only after existing commit holders have settled.
5. Drop the cold connection and all allocation/store clones before publishing
   WriterAbsent and removing the matching slot. Keep command success independent
   of cleanup success: a committed result with unproven close cannot be refired or
   reported as a rollback. Retain the slot, ownership and diagnostic if release
   cannot be proven. A successor writer is forbidden while that state remains.

Keep the shared retirement watch separate from the command receipt reply. Existing
Outcome::WriterAbsent/RetirementUnproven semantics can convey release; waiters on
the same cold flight must not receive another caller's command as their own result.
Extend the private terminal publisher with a storage completion path, without the
runtime candidate/activate path. Cancellation before commit returns no mutation;
cancellation after commit cannot erase its receipt. Neither caller drop nor panic
may fabricate writer absence. Registered cleanup remains part of the manager's
drain inventory until resource release is proven.

## Lock and wait phases

This is a phased order, not a requirement to hold every lock for the whole command.
The source's load path registers first and acquires allocation outside the map;
it does not already implement a complete Scheduler commit gate.

| Phase | Order / rule |
| --- | --- |
| Selection | Resolve catalog identity, release catalog mutex, then enter registry briefly to choose/register an owner. No SQLite or allocation open under registry. |
| Acquisition | Retain native allocation access after registration. Catalog/destructive exclusion revalidates liveness; no registry mutex is held while opening or waiting. |
| Mutation participation | Use ProductRoot's shared ownership-mutation participation before commit-phase registry/lifecycle/SQLite locks. Obtain it outside those mutexes. An ownership snapshot conflict yields/retries without a held commit permit. |
| Commit admission | Under registry, check closing/retiring state and increment the finite commit permit; release registry. For hot work, retain OperationLease plus native with_running_commit/publication; for cold work, retain the exclusive storage flight and its permit. Then enter SQLite. |
| Completion | Leave SQLite, hot publication/lifecycle and commit participation before terminal registry publication. Never acquire registry while retaining SQLite. Drop storage/allocation before notifying writer absence. |
| Delete | Set retiring_sessions and close further command/claim admissions under registry; release it before joining flights. Only after writer retirement acquire the existing root ownership freeze and sorted destructive allocation exclusions, then fresh preflight/catalog commit. |

The allocation lease may span phases; it is not permission to retain a catalog or
registry mutex while waiting. The root snapshot is nonblocking today: a concurrent
shared mutation must produce a typed conflict/retry, never a reverse lock wait.
The implementation must not wait for StorageOnly completion while holding that
snapshot, because completion may need to finish native ownership publication.
Model selection already takes application/catalog locks before its runtime lookup.
Cold commit/terminal publication must not call configuration or catalog helpers
under registry or SQLite; any post-commit catalog/display repair runs after those
locks have been released. This avoids adding the reverse edge to that existing path.

For host shutdown, preserve the existing host -> registry ordering used by
admit_request's synchronous admission callback. Close the manager's Scheduler
admission under that order when the host begins drain; the manager must not call
back into AppServer while holding registry. Join the driver and activation work,
counted commits and cold flights, preserve the host's final inventory pass, then
check complete emptiness. This is a private addition to existing ownership, not a
new distributed lock or a Scheduler-specific database.

## Required interleavings

| Race / frontier | Required result and evidence |
| --- | --- |
| load(A) versus cold write(A), both claim orders | One registry slot owns acquisition. The loser waits; no two existing-store writers. Load after cold success reads the committed receipt; failed close blocks load. |
| B resident, cold write(A) in the same Session | Cold A neither consumes B's runtime slot nor changes selected node. load(A) still refuses the B residency conflict. Retiring A leaves B intact. |
| Hot mutation versus unload/idle reap | OperationLease wins and is drained, or Unloading/idle claim wins and new admission is refused. A bare cached ManagedRuntime identity never bypasses either check. |
| Delete before cold registration | retiring_sessions prevents any allocation acquisition. |
| Cold registration before delete, before final commit permit | Delete sees the flight even though by_session lacks it; the final permit refuses, and cleanup proves release before deletion proceeds. |
| Commit permit before retirement request | Delete waits for that finite holder; fresh preflight sees its committed state/target or proven rollback. No commit follows established retirement. |
| Storage close failure/panic/caller drop | Report actual command receipt if committed, retain unproven ownership until native cleanup proves absence, and prevent replacement or deletion recovery from treating the Session as empty. |
| Drain during cold acquisition or a pre-drain RPC's late load | Stop new Scheduler commits, account for every registered flight, and use the final inventory pass. All successful-shutdown resource counts are zero. |
| Disabled/Completed task but pending Scheduler target | Exact native trigger evidence permits target-owner recovery; ordinary Human pending in another cold Session never causes Scheduler activation. |

Extend the existing [test probes](../tests/scripted/app_server/mod.rs) with cold
registered/before-open/before-commit/after-commit/before-close/after-release gates.
Reuse before_allocation, after_delete_fence, before_operation, before_shutdown and
after_writer_transfer for the competing side. Assert acquired-writer count,
matching slot/Session identity, owned operation count, durable receipt and final
allocation exclusion. Use real file-backed stores for release/exclusion checks;
do not substitute a second model of the registry and call it concurrency proof.

## Baseline verification and S1 acceptance

The existing tests already exercise important pieces of the intended seam:

- [deletion](../tests/scripted/app_server/deletion.rs): load claim visible before
  acquisition, deletion fencing, failed-loading absence, retained unproven writer,
  replacement handoff and deletion durability recovery;
- [replacement](../tests/scripted/app_server/replacement.rs): unload/load joining,
  native inbound-versus-drain winners, stale clients and writer transfer;
- [residency policy](../tests/scripted/app_server/residency_policy.rs): quota,
  operation lease versus idle/drain, failed slots, final supervision and host
  request/admission ordering.

Validation on 2026-10-22 in Linux/WSL passed the exact module filters under
`cargo test --offline --lib --all-features`:

| Filter suffix after `local_runtime::session_runtime_manager::tests::` | Result |
| --- | --- |
| `deletion::` | 15 passed |
| `replacement::` | 6 passed |
| `residency_policy::` | 18 passed |

These 39 baseline tests cannot cover StorageOnly yet. Documentation is checked
with `cargo test --offline --test contracts --all-features`.
S1 acceptance additionally requires every new interleaving above and the
plan's one-shot/backpressure transaction tests to execute against the implementation.

No additional interface investigation is needed before starting that bounded S1
slice. A failure to demonstrate writer absence, host closure or deletion fencing
must block shipping the slice; it cannot be hidden by SQLite CAS or a retry loop.
