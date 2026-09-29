# PR #425 lifecycle repair validation

This follow-up starts at reviewed HEAD `1ef85ca2827e55dcfd929fca010de47ea7238ee2`
in `/home/caismis/Documents/codes/rustX-issue-412`, on the existing
`issue-412-image-reading-bash-description` branch. Fetch confirmed main remained
`5d2e382154a9d3434afb860a6892d76e98ad6bfb`. That main commit is already an
ancestor and the merge base; the starting branch was four commits ahead, zero
behind. No further merge or conflict resolution was necessary. The primary
checkout and unrelated worktrees were not edited. The prior integration commit
is the reviewed HEAD; its App Server v29 and main Trace/Trajectory ownership
remain intact.

Baseline CI inspected: PR run `36496734021` and main run `36493246236`, both
successful. PR was open, mergeable and had no auto-merge request.

## Ownership and implementation

- `local_runtime/session/artifacts.rs` materializes all current typed canonical
  managed references: User Image/File, Assistant Image, Tool content Image/File,
  and Tool semantic artifacts. It scans the exact prepared cut, copies distinct
  immutable artifacts once through the source authority, allocates destination
  identities, and remaps only artifact references. The destination uses its
  existing ArtifactStore. Work is limited to 256 artifacts and 64 MiB total.
- `tools/artifacts.rs` streams settled artifacts under their existing reservation
  lock, checks the finite length, and syncs destination bytes and directory.
  `session.rs` performs this before SQLite lineage initialization/catalog
  publication; failures use ordinary private Session/node preparation cleanup.
  Session uploads retain their separate existing upload transaction.
- `model/images.rs`, `model/snapshot.rs`, and `agent/execution.rs` share one
  projection: newest 16 User/Tool image occurrences remain images; older blocks
  become textual artifact references. Repeated identities count as occurrences.
  Text-only invocations project all images. Canonical history is unchanged.
  Adapter validation also rejects more than 16 actual image blocks before HTTP.
- `context/images.rs` derives validated dimensions from the immutable snapshot
  and adds `1024 + ceil(width * height / 256)` tokens per selected occurrence to
  neutral text/structure accounting. Text projections have only text cost.
  The Attempt binds its exact frozen primary capability and artifact authority.
  Context fit, compaction and retention use this estimator. Observed prefix
  identity includes projected content, preventing reuse after image eviction or
  modality switching changes an otherwise identical canonical message ID.
- Web `nativeTools` includes `read_image` for root and named-Agent authoring;
  `policyTools` excludes it. Native configuration and rereads remain authoritative.
- `WorkspaceManager::git_raw` transfers its owned repository guard into the
  physical supervision task before spawn. Caller abort cannot release it while
  that task settles the Git child and drains output. The key remains canonical
  Git common-directory identity; unrelated repositories and non-metadata Git
  operations remain independent.

No protocol vocabulary changed in this follow-up. App Server stays **v29**, with
exactly `v29.ts` and `v29.schema.json`; generation also verifies native fixtures.
No generated artifact was hand-edited, created or removed. There is no obsolete
v27/v28 lineage, decoding fallback, migration or compatibility alias.

## Deterministic evidence and path audit

`managed_images_materialize_exact_clone_fork_branch_cuts` executes native
`read_image`, then modifies and deletes the source PNG. Clone, Fork and Branch
reopen their destination SQLite history and resolve the exact original bytes
through their own ArtifactStore. An unrelated source artifact forces a real
`artifact_2` to `artifact_1` remap. A pre-image cut copies none. Missing-source
artifact failure removes a partially prepared allocation; pre-rename publication
failure remains unpublished and is discarded. Ordinary source Session deletion
leaves independent Clone/Fork bytes readable. Same-Session Branch has independent
Conversation bytes and shares its Session's deletion lifetime.

`seventeen_parallel_image_reads_leave_a_valid_provider_continuation` uses the real
Agent Loop, native parallel Tools, Anthropic adapter, HTTP transport and external
provider emulator. Seventeen reads of the same file retain seventeen distinct
canonical identities. The next request contains exactly sixteen copies of the
original PNG payload plus a textual reference; it completes successfully.
Reopening SQLite reconstructs the same sixteen-image projection without encoded
bytes in durable request evidence.

Context regressions cover sixteen/ seventeen images, repeated identities,
text-only/image switching, unchanged canonical history, old-cut reselection,
SQLite reconstruction, projected-prefix invalidation, dimension-sensitive cost,
and pre-dispatch soft-limit/fit decisions. Existing provider-anchor tests remain
in the full suite. Existing image round-trip conformance covers text-only
switching, returning to images, summary/compaction, and Workflow delegation.

The source Conversation owns each canonical managed reference. Initial and
continuation requests resolve only its selected artifacts. Transient retries
reuse the admitted request; durable/overflow reconstruction repeats projection
from the frozen snapshot. Summaries use text transcripts. Independent Agent and
Workflow children start their own input rather than inheriting parent image
references. Archive traversal covers the same real artifact-bearing types and
reads destination-owned references after copying. Session deletion remains the
reclamation owner, including committed residue without a published reference.

The canonical image-deletion fixture still uses `ProductRoot::existing` and
`residue(root.root(), &preview)`. No confinement/access function changed.

The real-Git caller-abort regression blocks `update-ref` inside a native reference
transaction hook using FIFOs, proves physical entry, aborts the caller, directly
polls same-repository admission as pending, starts another independent manager,
and proves another repository can mutate. Only releasing the first helper lets
the second complete. No sleeps, retries, stderr matching or suite serialization
were introduced. Existing queued-cancellation and linked-worktree tests remain.

Web tests prove root select/save/authoritative reread/reload/remove, named-Agent
selection, and policy exclusion. Browser coverage loads and decodes copied image
artifacts through the supported Fork and Branch controls, plus reconnect/replay.
Web has no Clone command; Clone is covered natively. Existing browser acceptance
also exercises live image presentation and Bash command disclosure.

Anthropic remains the only image wire adapter, with static PNG User/Tool-result
placement. OpenAI Chat and Responses remain text projections. Unsupported
placements, invalid/unresolved bytes and over-budget raw requests fail before
HTTP. No formats, protocols, attachment stores or image conversion were added.
Bash description remains optional, nonblank, at most 160 Unicode scalar values,
and presentation-only: command/arguments retain execution and approval authority.
The prior deterministic Bash repair and unchanged production TERM grace remain.

## Validation

Final command results follow. Linux is the local
platform. No local macOS or credentialed live-provider result is claimed.

Development findings were fixed, not hidden: the first full Rust run exposed an
old startup fixture's nonexistent `artifact-1`; it now creates a real immutable
PNG while retaining its display-preview assertions. The new replay test initially
expected one request record instead of the scenario's two; only that incorrect
new-test count changed. Early browser test authoring attempted an unsupported Web
Clone command; coverage now uses the actual Fork/Branch controls. Early compile
and lint checks identified imports, borrows, Debug coverage and test formatting;
these were corrected. No timeout, retry policy, ignored test or CI configuration
changed to obtain passing results.

### Native validation results

| Command (repository root) | Final result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | pass |
| `cargo build --bins` | pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | 4,055 passed, 0 failed, 8 existing ignored |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,239 passed, 0 failed, 3 existing ignored |
| `cargo test --test contracts --test provider --all-features` | 195 passed, 0 failed, 5 existing ignored |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | 194 passed, 0 failed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 427 passed, 0 failed |
| `git diff --check` | pass |

The eight pre-existing ignored tests are five credentialed live-provider probes,
two isolated instrumentation/measurement tests, and the command that rewrites the
read-document fixture corpus. No ignore annotation changed.

Focused commands also passed:

```sh
cargo test --all-features local_runtime::session::tests::deletion_tests::lifecycle::session_deletion_reclaims_committed_image_without_a_published_reference -- --exact
cargo test --lib --all-features runtime::workspace::tests::concurrent_children_have_distinct_deterministic_paths_and_refs -- --exact --nocapture
cargo test --all-features --test tools bash::background_cancel_records_term_before_trap_and_physical_terminal -- --exact --nocapture
cargo test --all-features --test tools bash::bash_background_cancellation_uses_the_same_process_group_path -- --exact --nocapture
cargo test --all-features --test tools bash::bash_kill_escalates_when_term_is_ignored -- --exact --nocapture
cargo test --lib --all-features managed_images_materialize_exact_clone_fork_branch_cuts -- --nocapture
cargo test --lib --all-features caller_abort_keeps_metadata_gate_until_physical_git_settlement -- --nocapture
cargo test --lib --all-features context::images::tests -- --nocapture
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test conformance agent_loop::seventeen_parallel_image_reads_leave_a_valid_provider_continuation -- --exact --nocapture
```

Each selected one test except `context::images::tests`, which selected two. All
are also included in the final full Rust run. The external boundary counts are:
CFG3 catalog 26, managed output 5, conformance 24, durable 129, process 63,
subagent 45, Tools 135. Provider has 167 passing tests and contracts has 28.

### Packages and browser validation

| Directory and commands | Result |
| --- | --- |
| `test-support/fake-provider`: `uv sync --frozen`; `uv run --frozen pytest` | pass; 51 tests |
| `protocol/app-server`: `corepack install`; `pnpm install --frozen-lockfile`; `pnpm generate`; `pnpm check`; `pnpm typecheck` | all pass; no generated/fixture drift |
| `tui`: `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | all pass; 894 tests |
| `dev`: `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm test` | all pass; 37 tests |
| `web-console`: `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm check:i18n`; `pnpm check:provenance`; `pnpm build` | all pass; 145 provenance records and 131 package notices verified |
| `web-console`: `pnpm test test/settings.test.tsx` | 40 passed, including root and named-Agent image authoring |

Browser authority is the repository-pinned Playwright 1.63.0 Linux container,
using supported `CONTAINER_ENGINE=podman`; host fonts/browser caches are not used.
The focused `pnpm test:e2e test/e2e/settings.spec.ts test/e2e/chat.spec.ts test/e2e/commands.spec.ts`
run initially passed Settings and commands but exposed the unsupported Clone
command in the new test. The corrected focused Chat run passed, covering native
image load/decode, lightbox, continuation, reconnect, Fork and Branch.

The first full `CONTAINER_ENGINE=podman pnpm test:e2e` run passed 142/143; the sole
failure was the intentional named-Agent inventory screenshot change. Visual
comparison showed exactly the new `read_image` label replacing `ask_user` at the
bottom of the captured list (237 changed pixels), with unchanged geometry.
The reference is regenerated through the normal pinned-browser command, not
copied from an unverified host image or masked with a screenshot tolerance.

An additional overlapping `pnpm test` run hit the existing 5-second workspace
remount-test timeout and a subsequent request-list assertion (1,154 passed,
2 failed). An earlier full run of the same production code passed 1,156 tests.
No baseline-failure or scheduling-cause claim is made, and no timeout/assertion
was changed. Final standard runs after the heavy native work are recorded below.

Final `web-console/pnpm test`: **1,156 passed, 0 failed** (64 files).
`CONTAINER_ENGINE=podman RUSTX_SCREENSHOT_UPDATE=1 pnpm test:e2e test/e2e/settings-presentation.spec.ts --grep '390 × 844'`:
**1 passed**. Only `settings-agent-narrow-dark-linux.png` changed; the other
regenerated references are byte-identical.

Final `CONTAINER_ENGINE=podman pnpm test:e2e`: **143 passed, 0 failed** (7.2 minutes),
including the strict regenerated screenshot comparison. Formatting and whitespace
checks pass after the final documentation update. The final feature diff against
current main retains one protocol lineage, current Trace ownership, unchanged
confinement and Bash grace, and no sleep/retry/ignored-test/CI workaround.
