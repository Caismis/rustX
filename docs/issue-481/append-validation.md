# Connection-local append validation

This stage implements the explicitly authorized exception: a warm append need
not reread every historical identity solely to detect arbitrary out-of-band disk
corruption immediately. Native reads and explicit validation still read their
own authorities. A disposable proof never replaces Ledger, Pending, Journal,
checkpoint content, Original dependencies or the canonical frontier.

## Consumer audit

| Consumer | Validation path |
|---|---|
| Ordinary single/batch canonical acceptance | One provisional batch, native transaction, post-COMMIT seat |
| Canonical acceptance with event(s) | Same; events and observation staging remain in that transaction |
| Canonical publication (including HTTP assistant completion) | Same; publication remains after native COMMIT |
| Pending batch adoption | Same; each append retains the allowed Pending identity and native duplicate checks |
| Initialization/hidden context and lineage seed/bootstrap | Existing full validator/native bootstrap; writes invalidate any older proof |
| Current/historical reads, request construction, recovery, compaction and Fork | Existing materialization/selection authorities; no cached ID vector substitutes for them |

The former append validator checks frontier/publication, bounded ordered suffix
identities and checkpoint Original dependencies. Without a checkpoint it needs
only borrowed UTF-8 identity validation and a count, not message bodies. The
certificate stores that count, frontier, generation and invalidation tokens. It
contains no canonical messages or independent selected-history authority.

## Snapshot and mutation boundaries

The connection lease is held throughout. A batch removes any previously seated
proof before BEGIN, so early errors discard it. SQLite main data-version, main
schema-version and temp schema-version are read inside the immediate transaction
before each append, including subsequent appends in one batch. External writers
cannot cross the pinned writer snapshot. The native head/publication selector
still runs for every append; the certificate never authorizes a stale checkpoint
pointer or bypasses current publication integrity.

The shared SQLite preupdate callback dispatches to separate manifest and append
owners. The append owner watches Ledger insert/delete and changes to identity or
position, Surface head, and checkpoint/head/activation changes. Identity-neutral
Ledger body/display updates were not inputs to the original identity validator;
the existing manifest/read validators retain their own coverage. Journal and
bootstrap publication dependencies remain checked by the live head selector.
Unknown changes fail closed. Main schema changes disable reuse until reopen;
other version changes force full validation before establishing a new proof.

Advancement is allowed only after the native writer's expected two direct
mutations: one Ledger insert and one Surface-head update. Relevant nested or
unknown mutations advance the epoch by at least three, exceeding that expectation
even if a trigger suppresses/replaces a native write. Epoch arithmetic saturates;
maximum epoch permanently disables reuse. Rollback invalidates the epoch.

Only successful native COMMIT can seat the provisional proof. Seating performs
no fallible SQL after acceptance and cannot turn an accepted commit into an error.
An external write between COMMIT and seating is detected by the next transaction's
data-version check. Poisoned optional cache state falls back to validation.

## Preserved semantics and limits

Count is independent of physical Ledger position, preserving sparse frontiers.
The existing pre-append 8,192 bound still permits the last append to 8,193 and
rejects the next; no audit/history rows are discarded. The writer still checks
Ledger and Pending uniqueness/order. A new identity wider than 4,096 bytes keeps
its existing admission/error timing: it receives no reusable proof, and the next
append applies the original validator. Checkpoint Originals already exist in
Ledger, so an admitted fresh unique identity cannot duplicate them. Checkpoint
changes or Original identity deletion invalidate the proof and require full
materialization. Actual checkpoint/publication parsing remains unavoidable work.

This is not whole-request O(1): context construction, Original selection,
checkpoint validation, native writes, events and optional observations retain
their costs. No background persistence queue, new cache authority, persistence
format change, migration, compression or publication-cadence change is introduced.
WAL/NORMAL's healthy-OS process-crash contract and ordered P<U<C commits remain.

## Validation checkpoint

All 383 durable SQLite tests pass, including the new certificate cases. The
all-target/all-feature Clippy pass follows a semantics-preserving match-arm
cleanup and a documented allowance keeping the atomic publication transaction
in one function. The final focused rerun passes all 15 append tests, including
added sparse-frontier warm reuse and an external writer placed deterministically
between COMMIT and seating. No sleeps, retries or timing-based assertions are used.
The first focused fixture expectation failure and first three lint findings are
retained in evidence. Exact final combined FULL/Merge validation remains required.
