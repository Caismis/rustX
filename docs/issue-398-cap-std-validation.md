# Issue #398 — cap-std architecture validation

## Decision: No-Go

Retain the bounded native `std + nix` upload implementation. Do not begin R4.
cap-std is a useful containment mechanism, but containment alone is weaker than
rustX's full-path no-symlink policy. Repairing that difference retains a custom
component walk, native authorized bootstrap, native publication validation,
independent locks and durability/commit owners. The experiment adds conversions,
readable-directory reopening and dependencies without removing those concepts.
This is a decision about this pilot and these contracts, not a library defect or
an assertion that a process sandbox is needed.

Production filesystem implementation was not migrated by this PR.

## Audited repository and scope

Audit/base: `7880d1b57d4ff9627fa8f567d75b50afa3178385`, fetched from
`origin/main`, not inferred from the issue's older planning revision.
Branch: `issue-398-cap-std-validation`; worktree:
`/home/caismis/Documents/codes/rustX-issue-398`.
The original `main` worktree contained untracked `.playwright-mcp/`; it and all
other worktrees were left alone. Issue #398 had no comments at audit. No matching
398/cap-std branch or PR existed; the repository had no open PR at the subsequent
check. Recent history reviewed includes merged #387/#390 storage work, #396/#403
CLI dependencies, #397/#405 dependency/parser work, and #402/#404 Session recovery.
The existing documentation convention is `docs/issue-N-validation.md`.

Only `uploads::cap_std_validation` is a candidate implementation, included under
`#[cfg(test)]`. Two `#[cfg(test)]` native sync checkpoints exercise the existing
owner; ordinary builds contain neither checkpoint nor candidate. The candidate
is not a production selector, feature, facade, VFS or public API. No reservation,
SQLite, catalog, deletion, artifact, settings or process implementation is changed.

## Ownership inventory and exact boundaries

| Boundary / source | Actual owner and responsibilities |
| --- | --- |
| `runtime/local_storage.rs`: `ProductRoot::{create,existing,confined}` | Trusted startup root acquisition, canonical product identity, private allocation path policy. **Not a live storage-access guard.** `existing` is noncreating; `create` creates root/namespace and establishes directory barriers. |
| Same file: `ProductController::acquire` | Exclusive controller admission on independently opened `.product-writer.lock`; creating startup metadata is intentional. |
| Same file: `OwnershipSnapshot`, `OwnershipMutation`, `runtime_ownership_admission` | Freeze/participate in ownership transitions on independently opened product-root lock objects. Existing acquisition order and lifetime are domain semantics. |
| Same file: `ConversationAccess::{existing,start}`, `ConversationExclusion::acquire` | Shared/exclusive allocation access, catalog liveness admission, destruction exclusion. `existing` locks before `SessionCatalog::check_allocation_live`. |
| `local_runtime/session_controller.rs`: `SessionController::upload`, `acquire_session` | Preparation lock, live Conversation admission, canonicalize `access.settings.cwd`, durable claim, native ready commit, then Session-scoped receipt. Workspace and ProductRoot are distinct. |
| `local_runtime/session/uploads.rs`: `validate_name`, `validate_workspace`, `stable_directory`, `directory_at`, `session_directory` | Input policy, no-follow full-path/component traversal, upload-directory creation. These are not authorization substitutes. |
| Same file: `UploadRegistry::{claim,materialize,verify_materialized,receipt_ref,receipts}`, `SessionCatalog::commit_uploads` | Allocation ownership; exclusive creation; contents and directory barriers; reopened batch dev/ino; regular-file checks; registry publication and receipt gating. |
| `local_runtime/session.rs`: `SessionCatalog::commit`, `atomic_write`, `sync_directory_ancestry` | Native catalog serialization, temporary file, file sync, visibility rename, ancestry sync and explicit uncertain-durability errors. |
| `local_runtime/session/deletion.rs`: `snapshot_preflight`, `commit_delete`, `CleanupWork::run`, `finish_delete`, `check_allocation_live`; `uploads::cleanup` | Ownership snapshot, sorted Conversation exclusion, frozen deletion record/workspaces, cleanup and terminal commit. Capability possession cannot override any of these. |
| `tools/artifacts.rs`: `ArtifactStore::{new,create_artifact,open_writer_inner,open_archive_reader,put_bounded}`, `with_lifecycle` | Conversation identity and lifecycle admission, monotonic reservation/capacity, file policy, bounded reads, durability and writer admission. Some traversal overlaps; these domain semantics do not. |
| `skills/source.rs`, `skills/package.rs`; `runtime/resources.rs::validate_project_resource_path`; `local_runtime/resource_directory.rs::{entries,read_resource}` | Authorized User/Workspace resource identity, canonical paths, no-symlink/regular-file policy, package validation and source selection. Inspection must remain inert; visibility is not filesystem permission. |
| `local_runtime/configuration.rs` root resolution; `local_runtime/settings.rs::{lock_document,read_document}` | Canonical configured roots, disjoint runtime-root/Workspace checks, authored source policy, document locks and settings revisions. Not a reason for an unrelated settings rewrite. |
| `durable` / `SqliteConversationStore::{open,open_existing}` | SQLite opens a pathname independently and owns its own journal/database I/O. Native access admission must precede it. A parent `Dir` does not govern SQLite. |
| Tool/process supervisors, managed Python, MCP and subagent launch paths | Executable, cwd and resource paths cross into subprocess APIs. Launch/lifecycle permission remains native. Child/restarted processes acquire their own authority. |

## Root acquisition, inputs and threat model

ProductRoot startup canonicalizes the supplied root in `ProductRoot::existing`.
Existing root aliases and ancestors are allowed at this trusted identity boundary;
missing/dangling roots fail. `confined` rejects symlinks below the canonical root,
including inside-root and dangling links, and permits missing identity-derived
leaves. It is a path-policy check, not the upload owner's descriptor-relative
full-path traversal. This ADR does not promote all private-path callers to the
upload pilot's race guarantees.

Upload root acquisition is `SessionController::upload`: acquire live Session
access, canonicalize configured cwd, require UTF-8, then claim that Workspace.
Thus configured root aliases may resolve inside or outside their original textual
parent at the trusted configuration boundary. The resulting **canonical spelling**
is traversed from `/` using `stable_directory`; each normal component, including
ancestors above Workspace, Workspace itself, `.agents`, `uploads`, Session and
batch, is opened with directory/no-follow/CLOEXEC flags. Subsequent symlink
substitution fails closed. Dangling configured roots fail canonicalization.
Canonicalization alone is not cited as race safety: the subsequent checked
native descriptors are the actual operation authority.

Inputs are different classes:

* Session/batch/token identities are native identity-derived components.
* Upload basenames pass `validate_name`; confinement does not replace its
  portability, reserved-name, separator or control-character rules.
* Configured Workspace paths undergo native configuration and live Session
  admission, then trusted canonicalization; they are not ProductRoot children.
* Arbitrary user paths cannot invoke this upload mechanism as an authority
  upgrade. The candidate receives a `Dir` made with `from_std_file` from the
  already acquired native descriptor. It never calls `open_ambient_dir`.

The admitted integration fixture retains `ConversationAccess` through claim,
materialization and native registry commit, consumes its directory capability in
materialization, and drops access afterward. No cached capability survives
revocation as a second permission system. Raw capability-only unit fixtures are
explicit mechanism probes, not a public authorization boundary.

Concurrent rename can detach an open object from its original pathname; retained
object access does not prove publication identity. Symlinks can redirect lookup;
inside-root symlinks still violate policy. Hard links are different: neither
no-follow nor cap-std establishes exclusive provenance for a regular inode with
multiple names. Mounts may expose other storage beneath a directory; this pilot
does not prohibit mount crossing or validate mount provenance. A same-UID process
can modify writable trees and rename them after a validation point; neither
implementation establishes isolation from every such mutation.

SQLite, subprocesses, independently launched processes, raw-path APIs and restart
are outside a parent capability's automatic control. cap-std does not sandbox the
rustX process. Cooperative native controllers and the existing permission model
remain assumptions. No exploit or independently demonstrated production defect
is claimed by the candidate's API mismatches.

## Library admission and selected-version source

Exact dev dependencies: `cap-std = 4.0.3`, defaults disabled (default is empty);
`cap-fs-ext = 4.0.3`, defaults disabled, `std` enabled. No direct cap-primitives
dependency. Extension admission is justified by the demonstrated absence of a
high-level final-no-follow switch on `Dir::open_dir`/ordinary `OpenOptions`.
`DirExt::open_dir_nofollow` and `OpenOptionsFollowExt::follow` supply that narrow
gap; they do **not** eliminate the full-path walk.

Sources inspected from the downloaded exact-version crates and upstream:

* [cap-std 4.0.3 Dir](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html):
  `from_std_file`, `open_with`, `open_dir`, `create_dir_with`, `try_clone`,
  `into_std_file`. `try_clone` delegates to `std_file.try_clone()`.
* [cap-fs-ext follow contract](https://docs.rs/cap-fs-ext/4.0.3/cap_fs_ext/trait.OpenOptionsFollowExt.html):
  `src/open_options_follow_ext.rs` explicitly says last component.
  `src/dir_ext.rs::open_dir_nofollow` delegates to cap-primitives.
* [cap-primitives 4.0.3 source](https://docs.rs/crate/cap-primitives/4.0.3/source/src/):
  `fs/open_dir.rs` and `rustix/fs/oflags.rs` select `O_PATH` for ordinary directory
  capabilities on Linux, add CLOEXEC, translate create-new to exclusive creation
  and translate no-follow to final-component `O_NOFOLLOW`.
  `rustix/linux/fs/open_impl.rs` uses `openat2` with `BENEATH | NO_MAGICLINKS`,
  not recursive `NO_SYMLINKS`, with manual traversal fallback.
* [flock open-file-description semantics](https://man7.org/linux/man-pages/man2/flock.2.html).

Context7 was attempted twice with cap-std and Bytecode Alliance-specific queries;
it returned unrelated projects, so no unrelated documentation was used. Exact
crate source and official docs were the fallback.

Both direct crates and cap-primitives use `Apache-2.0 WITH LLVM-exception OR
Apache-2.0 OR MIT`. Upstream was unarchived, last pushed 2026-08-20, observed
2026-09-25; its workflow includes Linux/macOS/Windows and cross-target checks.
This is maintenance evidence, not a future support guarantee. rustX's present
Unix code and CI support Linux/macOS; this experiment does not add Windows support.
Linux openat2 and macOS/manual traversal are distinct implementations. Only actual
local Linux execution is claimed below; macOS is selected by existing CI.

The direct crates do not declare a numeric MSRV. A separate minimal manifest with
these exact dependencies/features successfully ran `cargo +1.92 check --locked`
on Linux. This verifies candidate dependency compilation, not the whole rustX
MSRV or every target. rustX `rust-version = "1.92"` is unchanged.
`unsafe_code = "deny"` remains unchanged; this PoC contains no unsafe blocks or
allow bypass. Dependency internals do contain unsafe descriptor conversions and
OS interop: safe rustX source does not mean an unsafe-free dependency closure.

Lockfile delta: **11 added package/version pairs**, no existing version updates:
`ambient-authority 0.0.2`, `cap-fs-ext 4.0.3`, `cap-primitives 4.0.3`,
`cap-std 4.0.3`, `fs-set-times 0.20.3`, `io-extras 0.19.0`,
`io-lifetimes 2.0.4`, `io-lifetimes 3.0.1`, `maybe-owned 0.3.4`,
`rustix-linux-procfs 0.1.1`, `winx 0.36.4` (Windows-specific).
Already-present rustix/libc/windows dependencies are reused. The duplicate
io-lifetimes major versions are included in this cost. No production dependency
edge to these candidates is added; `cargo tree --locked --edges normal --prefix
none` confirms both direct candidate crates are absent from the normal tree.

## What the executable comparison proves

All new test names below are in
[`uploads/cap_std_validation.rs`](../src/local_runtime/session/uploads/cap_std_validation.rs).
Tests assert expected mismatches rather than weakening normative baseline tests.

### Full-path policy

`full_path_symlinks_distinguish_policy_from_confinement` builds separate declared
paths for ancestor-above-Workspace, Workspace root, intermediate upload directory
and leaf directory, each with inside/outside/dangling targets. Native
`stable_directory` rejects all raw symlink spellings. Unwrapped cap-std follows
inside-root links, refuses escapes, and fails on dangling targets. Final-only
no-follow still accepts an inside-root ancestor followed by further components.
`leaf_types_match_native_verification` separately covers file leaves and all
three target classes. The repaired candidate must retain a component walk with
one-component no-follow calls; no canonicalize-then-unchecked-open handoff is used.

### Publication and deterministic replacement

`native_commit_failures_and_candidate_substitutions` uses a synchronous callback
as a precise gate: batch capability opened → callback entered → Workspace,
`.agents`, `uploads`, or batch renamed to `parked` and a fresh declared tree
created → callback returns → write resumes through original capability → native
path reopen/dev+ino comparison fails. Bytes are asserted in the parked object,
the substituted tree is not adopted, allocation stays present/not ready, and
native `receipts` refuses a usable receipt. The leaf variant substitutes a
symlink after sync and before verification and fails regular-file/path policy.
No scheduler timing or sleeps establish these sequences.

`replacement_before_component_open_fails_closed` establishes acquired parent →
rename next directory → install inside/outside/dangling link → native and candidate
single-component opens reject. The existing
`ancestor_swap_before_readiness_fails_closed_and_remains_owned` exercises the
**real controller upload** gate after materialization and before ready verification.
The new admitted harness composes existing native claim/commit owners with the
candidate; it is not evidence of a production candidate integration. It is a
single-request fixture, not a replacement for the controller preparation mutex;
existing real-controller tests retain serialization and deletion coverage.

A single dev/ino check is a publication-boundary observation. Production
`materialize` compares batch identity; after the existing upload gate,
`verify_materialized` reopens and checks file types. It does not repeat the earlier
batch dev/ino comparison. Neither the earlier comparison nor possession of a
capability prevents later external rename. No stronger continuous guarantee is
claimed, and no production publication change was made.

### Locks and Linux descriptor suitability

`same_process_locks_and_clone_counterexample` tests shared/shared success,
shared/exclusive, exclusive/shared and exclusive/exclusive nonblocking contention,
EWOULDBLOCK, and independent guard drop. Two native-readable descriptors derived
by `Dir::try_clone`, however, share the open-file-description: both exclusive
locks succeed and dropping guard A explicitly unlocks guard B's lock. This is an
expected library/OS semantic, not a bug. Do not use cloned root descriptors as
independent native ownership guards.

`cross_process_locks_and_drop` launches this lib test binary's `lock_child`.
Child acquires → writes/flushed `ACQUIRED` → parent checks both request modes →
parent sends byte A → child drops A/flushed `DROPPED_A` → parent confirms shared
B still excludes exclusive acquisition → parent sends B → child exits → parent
can acquire exclusive. For an exclusive child there is one guard, so dropping A
releases it. Existing local-storage child-kill tests retain real death/restart,
alias and independent-root coverage. No fake process boundary is used.

`default_directory_handles_are_not_sync_or_lock_handles` (Linux only) records
EBADF for sync/flock on `open_dir`'s O_PATH handle. A duplicate does not repair it.
The candidate explicitly reopens `"."` with readable directory flags through the
retained capability, then uses safe `File`/nix interop. This also supplies separate
open-file-descriptions for logical locks. macOS does not run the Linux-specific
O_PATH assertion; it runs the portable contention/lifetime tests.

### Durability

The native claim is committed before creating upload state. Child/parent
initialization barriers, exclusive batch creation and parent sync, file writes
and file sync, batch/session directory sync, publication validation and native
ready-registry commit remain distinct steps. The candidate's explicit checkpoint
trace is `opened`, `file sync`, `directory sync`, `publication`; actual directory
initialization and parent barriers also occur in the candidate around that trace.
Visible bytes are not a durable receipt.

`native_sync_failures_never_commit_ready` injects errors at the existing native
file/final-directory sync boundaries and calls actual `SessionController::upload`.
The candidate admitted harness independently injects file-sync, directory-sync
and final-publication failures. All leave a native durable claim, not-ready state
and no receipt. For registry failure the harness uses the existing
`arm_write_fault_before_rename` seam in **native** `commit_uploads`; complete synced
bytes still cannot produce a receipt. Existing catalog tests cover
`CommittedButDurabilityUncertain` after rename; the PoC does not redefine that
outcome as rollback.

**Injected sync failures prove control flow and commit ordering, not real
storage-device power-loss behavior.** No physical power-loss test was run.

## FS-01–FS-12 acceptance matrix

`U` means `local_runtime/session/uploads.rs` and its existing `tests.rs`;
`S` means `runtime/local_storage.rs`; `C` means `session_controller.rs`;
new tests are in `cap_std_validation`. “Preserved” refers to the bounded repaired
test candidate, not approval for migration or a claim about all filesystem users.

| ID | Production owner / baseline test | Candidate test | Exact synchronization | Observation and limitation |
| --- | --- | --- | --- | --- |
| FS-01 | U `validate_name/validate_workspace`; `names_and_symlink_ancestors_fail_closed` | `path_policy_and_read_only_inspection` | Sequential identical fixture; exclusive missing-leaf create then repeat | Escape/absolute open rejected; native invalid-component/name policy retained. Internal `..` is allowed by cap containment, so it cannot replace native policy. |
| FS-02 | U `stable_directory/directory_at`; baseline inside/outside/dangling calls in new comparative test; S `native_private_storage_rejects_symlinks_before_creating_stores` | `full_path_symlinks_distinguish_policy_from_confinement`, `leaf_types_match_native_verification` | Install each link before lookup | **Unwrapped candidate mismatch:** inside-root ancestor follows, even with final-only no-follow. Repaired candidate requires native walk plus component calls. Trusted canonical root aliases are distinct from forbidden traversal links. |
| FS-03 | U/C `ancestor_swap_before_readiness_fails_closed_and_remains_owned` | `native_commit_failures_and_candidate_substitutions`, `replacement_before_component_open_fails_closed` | Existing `upload_commit_gate`; candidate opened callback → rename/substitute → resume; parent-open → replace → next-open | Root/ancestor/intermediate/batch/leaf sequences fail closed at appropriate boundary. Not a stress test or proof against all possible interleavings. |
| FS-04 | U `exclusive_batch_creation_never_overwrites_and_cleanup_never_follows_child_links` | `exclusive_creation_modes_types_and_cloexec`, `path_policy_and_read_only_inspection` | Two workers and main rendezvous on barrier for batch, then independently for file | One legitimate create winner; loser AlreadyExists; existing residue not adopted; symlink create-new fails. Random identifiers remain native. |
| FS-05 | U `materialize/verify_materialized`; comparative `leaf_types_match_native_verification` | `exclusive_creation_modes_types_and_cloexec`, `leaf_types_match_native_verification` | FIFO has no writer; nonblocking open returns before type rejection; fcntl reads FD flags | 0700/0600, regular-file rejection, inside/outside/dangling links, FIFO, directory and CLOEXEC asserted. Umask is not changed. Arbitrary device types are not exhaustively enumerated. |
| FS-06 | U/C native sync barriers and ready commit; `native_sync_failures_never_commit_ready`; catalog atomic-write fault tests | `native_commit_failures_and_candidate_substitutions`, Linux O_PATH probe | Exact sync/publication checkpoints; existing catalog BeforeRename injection | No ready/receipt after file, directory, publication or registry failure; native sequence retained. Linux raw Dir needs readable reopen. No power-loss proof. |
| FS-07 | U `materialize` reopened dev/ino; existing controller ancestor-swap test | `native_commit_failures_and_candidate_substitutions` | Open batch → substitute real directory → write retained object → reopen declared path | Bytes stay with retained object, identity mismatch prevents commit/receipt. Does not prevent post-check rename. |
| FS-08 | S independent `directory/lock`; `cross_process_target_conflicts_aliases_death_and_independent_roots` | `same_process_locks_and_clone_counterexample`, `cross_process_locks_and_drop`, Linux O_PATH probe | Same-process ordered calls; real child stdout/stdin handshake and wait | Independent reopen preserves four contention cases and drop lifetime; cloning is **not** independent ownership. Existing locks remain native. |
| FS-09 | S access/liveness; C admission; U `commit_gate_receipts_restart_concurrency_and_failed_turn_are_independent`, deletion tests | `identity_is_not_lifecycle_authority`; admitted candidate harness | Hold native exclusion then attempt access while Dir still exists; two ownership guards then drop A | Capability can still stat while admission is rejected: it is deliberately not authorization. Candidate consumes Dir before access drops. No revocation framework added. |
| FS-10 | S `management_lock_lookup_is_noncreating_and_paths_fail_closed`; `ProductRoot::existing` | `path_policy_and_read_only_inspection` | Snapshot empty fixture → missing native/candidate lookups → count unchanged | No state created by inspection; creation happens only in explicit create step. Does not generalize every resource reader to upload policy. |
| FS-11 | Native SQLite, process, identity and lifecycle owners in inventory | Source/API audit and threat model above | No claimed synchronization can sandbox unrelated path consumers | Limits for SQLite, subprocesses, raw paths, restart, hard links, mounts, rename and same UID documented. No sandbox claim. |
| FS-12 | U native `materialize` + `verify_materialized`; #387 evidence conventions | `same_environment_measurement`; dependency/source audit | Alternating order, five repetitions, separate fresh identical fixtures | Measured operation cost, 11-package delta, retained semantics/interop; **net simplification not established**. Timings are not syscall/device counters. |

## Complexity comparison

No production code is removed; adoption is intentionally not implemented.
The comparison baseline is the real private upload helpers and `materialize`, not
a shortened illustrative implementation. The candidate replaces mkdir/open/write
mechanics with `Dir` methods, but retains native `validate_name`,
`validate_workspace`, identities, authorized `stable_directory`, path construction,
registry validation, allocation claim, publication re-open/dev/ino, type checks,
receipt validation, catalog commits and cleanup/lifecycle. It retains nix for
independent flock and file-descriptor assertions, and uses File for directory sync.

The test-only candidate adds `mkdir`, `create`, `regular`, `sync_dir`,
`readable_directory`, a materialization comparison, conversions and error
propagation. It retains its own three-component directory walk and each
child/parent sync. On Linux, readable reopening adds operations because ordinary
Dir handles are unsuitable for sync/flock. Native cleanup, copies, private storage,
artifacts, packages/settings, SQLite and subprocess paths remain untouched.
The test orchestration and fault/handshake helpers are evidence costs, not claimed
production savings. No generic abstraction was introduced to hide those costs.

Removing a few openat/mkdirat expressions does not remove policy or ownership.
Two capability crates plus nine transitive package/version additions, the
O_PATH/readable distinction, final-only/full-path distinction and
clone/independent-lock distinction increase the number of facts a maintainer must
keep correct. Retaining a full custom walk around cap-std is not material net
simplification for this bounded upload slice.

## Measurements and validation

Measured executable revision: `0deaeae8bd3d0cca996d734bc2dfdaf577e3c668`
(the following ADR commit changes documentation only). OS: Fedora Linux 44,
x86_64; kernel `7.2.7-200.fc44.x86_64`; Intel Core Ultra X7 358H, 16 logical CPUs,
30 GiB RAM. Compiler: `rustc 1.95.0 (59807616e 2026-04-14)`, LLVM 22.1.2.
Profile: ordinary unoptimized Cargo test profile with debuginfo, all features.
Candidate: cap-std/cap-fs-ext 4.0.3. No CPU affinity, cold-cache control or host
isolation; compilation activity can introduce noise.

Each side performs 20 exclusive batches, one identical 4,096-byte file per batch,
five repetitions, fresh equal-depth fixtures, alternating baseline/candidate
order. Fixture setup and native claim construction are outside the timer.
Native `materialize` plus `verify_materialized` is compared with the test
candidate's materialization, native bootstrap/publication re-open and candidate
regular-file checks. Both complete bytes and all prescribed file/directory
barriers. Registry/controller work is excluded on both sides: this is **not** a
Session-create, ready-commit or end-to-end upload benchmark. The candidate checks
types using the retained batch capability, whereas baseline verification reopens
the path again; the admitted contract harness additionally invokes native
verification before commit. This difference is included in the interpretation,
not presented as equal internal syscall workloads.

Raw elapsed microseconds for each **20-batch repetition**:

| Filesystem | Baseline reps 0–4 | Candidate reps 0–4 | Baseline median (range) | Candidate median (range) |
| --- | --- | --- | --- | --- |
| `/tmp`, tmpfs (`rw,nosuid,nodev,seclabel,inode64,usrquota`) | 4578, 954, 1035, 892, 873 | 2959, 1131, 1104, 1069, 1074 | 954 (873–4578) | 1104 (1069–2959) |
| `/home`, btrfs `/dev/nvme0n1p3[/home]`, `compress=zstd:1,ssd,discard=async,space_cache=v2` | 28408, 26224, 25378, 25126, 25385 | 24304, 29115, 26129, 25573, 24930 | 25385 (25126–28408) | 25573 (24304–29115) |

Median amortized time per batch: tmpfs 47.7 vs 55.2 µs; btrfs 1269.25 vs
1278.65 µs. These ranges do not establish a performance advantage. tmpfs sync
costs do not represent durable physical media; btrfs flush completion is not a
power-loss experiment. No syscall counts, physical I/O counts, or #387 logical
counters are reported as such. Architectural complexity, not speed, decides No-Go.
No controlled cold-build delta is claimed; the dependency/package delta above is
the reproducible build-input cost.

Reproduce the measurement (the test normally also runs in CI, without timing
thresholds):

```sh
cargo test --lib --all-features --locked cap_std_validation -- --nocapture
cargo test --lib --all-features --locked same_environment_measurement -- --nocapture
mkdir -p target/issue398-measurement
TMPDIR="$PWD/target/issue398-measurement" cargo test --lib --all-features --locked same_environment_measurement -- --nocapture
```

The recorded isolated measurements invoked the compiled test binary directly:
`target/debug/deps/rustx-869e16816e2daa3c --exact
local_runtime::session::uploads::cap_std_validation::same_environment_measurement
--nocapture`, with default TMPDIR and then the btrfs TMPDIR above. Cargo's binary
hash varies with environment. Test timing excludes compilation in either form.

MSRV admission probe: create a separate minimal edition-2024 package declaring
`rust-version = "1.92"`, the two exact dev dependency declarations above as its
normal dependencies, and `fn main() {}`. Resolve its lockfile, pin shared versions
to this repository's `bitflags 2.13.1`, `ipnet 2.12.1`, `rustix 1.1.4`, then run
`cargo +1.92 check --locked --manifest-path /tmp/issue398-msrv/Cargo.toml`.
It passed; all dependency package/version pairs then match this repository's
lockfile. No claim is made that undeclared upstream MSRVs are contractual.

### Validation and CI selection

| Command | Result |
| --- | --- |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo check --all-targets --all-features --locked` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --locked` | Pass |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | Pass |
| `cargo test --test contracts --test provider --all-features --locked` | Pass |
| `cargo test --lib --all-features --locked -- boundary_suites::` | Pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Pass |

Unit selection: 3,099 passed, 2 existing ignored. Contracts: 28 passed. Provider:
166 passed, 5 existing opt-in ignored. In-crate boundaries: 225 passed. External
boundaries: cfg3_catalog 26, cfg3_managed_output 5, conformance 22, durable 129,
process 62, subagent 43, tools 130 passed (417 total). No failures. The 13 new
probes are included in the unit selection and also passed separately with
`cargo test --lib --all-features --locked cap_std_validation -- --nocapture`.
The required commands use `--locked` even where current CI omits it. The existing
fake-provider environment was prepared with `uv sync --frozen` and
`uv run --frozen pytest` (51 passed).

All 13 new tests passed locally on Linux, including real cross-process locking.
The Linux-only O_PATH probe intentionally does not run on macOS. Existing CI
`rust-contracts` selects this module with `--lib ... --skip boundary_suites::`;
`rust-platform-boundaries` selects it on macOS with `--lib --bins ... --skip
scripted_suites:: --skip local_runtime::session_runtime_manager::tests::`.
No new external target or workflow change is necessary. macOS execution was not
available locally; CI selection is not reported as an observed macOS pass.


## Finite recommendation

Keep the existing native implementation and retain these dev-only counterexamples,
contract comparisons and ADR. Do not create a migration issue or implement R4.
Reconsider only with a separately bounded proposal that actually removes full-path
traversal/interop complexity while preserving native owners; this decision makes
no repository-wide filesystem-library commitment.
