# PR #320 review corrections

This supersedes the [initial issue validation](issue-319-validation.md) for the
reviewed head `8701fd31aba4add200576df75b53d4d903ed8a15`. The same non-draft PR
and branch are retained. Fetched base `origin/main` remains
`e256998fda8d0fa56c47b8623e2c97d2eca46836`; no upstream integration was needed.
There were no newer branch commits or inline review threads when work began.
A later automated inline review about concurrent file selection was also inspected
and addressed before pushing.
The submitted review's five findings are addressed below.

## Ownership corrections and deterministic evidence

1. **Private preparation recovery.** `copy_required` only materializes bytes.
   `SessionController` owns failure cleanup; the catalog consumes its existing
   preparation claim only after upload roots and the identity-derived private
   Session directory are removed and synced. Cleanup failure retains the claim
   and returns both operation and cleanup errors. Startup retries exactly that
   frozen claim. Empty workspace lists still own the private native allocation;
   pending claims reserve Session identities independently of filesystem residue.
   `failed_copy_cleanup_retains_the_frozen_claim_until_recovery_finishes` deletes
   the second source file, observes the first destination file, forces cleanup
   failure, checks invisibility and retained authority, retries under the fault,
   verifies identity reservation, then reopens and verifies recovery and source
   preservation.
2. **Branch cleanup.** Independent copies discard the private Session and uploads;
   same-Session branches discard only their unpublished Conversation directory.
   `branch_publication_faults_preserve_commit_semantics_and_only_discard_private_nodes`
   parks at the publication Gate, injects pre-/post-rename catalog faults, proves
   the original `NotCommitted` error and no orphan in the first case, and proves
   authoritative committed node visibility plus durability diagnostic in the
   second. Original and unrelated Session files survive both cases. Local attached
   fork/tree commands now use the same controller lifecycle with their admitted
   historical source; they no longer bypass upload preparation.
3. **Core resolution boundary.** `model::uploads::UploadProjectionResolver` is the
   finite read-only capability consumed by Context and its estimator. Local
   Runtime implements it with `SessionUploadResolver`; only that implementation
   knows Session catalog persistence. The XML renderer is unchanged.
   `context_estimates_the_same_rendering_as_provider_input` exercises the new
   boundary and checks identical estimates. The scripted upload projection test
   changes current workspace metadata after snapshot capture and verifies
   reconstruction still uses only frozen request paths. A source audit finds no
   `local_runtime` or `catalog.json` in `src/context`.
4. **Faithful editor restoration.** The controller unions canonical copied-lineage
   uploads with the selected editor boundary uploads before independent copying.
   It converts the exact ordered canonical editor facts into native
   `UserInputBlock` text/receipt input owned by the destination. Same-Session
   branches share receipts without copying. The schema/TypeScript and
   native transition types now express that contract. TUI retains restored receipts
   alongside the exact body and sends them on admission; it shows the attachment
   count and never assumes a client-local path exists on the server.
   `restored_editor_uploads_are_ordered_owned_and_prepared_before_publication`
   gates both independent/same-Session cases with `[A, text, B]` and checks order,
   bytes, source-deletion independence and cross-Session rejection.
   `restored_destination_receipts_admit_a_turn_after_source_deletion` submits the
   returned editor over App Server after deleting the source and checks the actual
   provider request. `tui/test/editor-upload.test.ts` checks unchanged and edited
   bodies retain ordered receipts.
5. **Web uncertainty.** `isOutcomeUncertain` includes transport outcome loss and
   typed `committed_durability_uncertain`. InputBar disables Send for that draft,
   presents reconciliation guidance, and never replays. The client/presentation
   regression delivers the typed RPC error and proves there was exactly one
   upload request and no turn submission.

The later inline review identified silently ignored file/drop/paste selections
while a transfer was active. Each is now explicitly refused with actionable draft
feedback. Three promise-gated UI cases prove the refusal, no extra upload, and
successful deliberate selection after the first upload completes.

All new race/failure regressions use catalog faults, Gates, explicit cleanup fault
injection or filesystem fixtures. No sleeps or weakened liveness guards were added.
Upload readiness, canonical commit authority, the single XML renderer and frozen
`RequestSnapshot.upload_projection` remain unchanged. No new persistence registry,
provider adapter, storage backend or compatibility path was added.

## Validation

The final `.github/workflows/ci.yml` defines these commands. All final runs passed.
Local execution is Linux; macOS execution belongs to the PR's separate CI lane.
The existing production bundle-size advisory is non-fatal. No environment failure
was waived, and mandatory emulator checks were enabled.

| Directory | Exact commands | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | Pass |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| root | `git diff --check`; `git diff --cached --check` | Pass |
| root | `cargo build --bins` | Pass |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,849 passed, one existing ignored; bins/examples passed |
| root | `cargo test --test contracts --test provider --all-features` | 25 contracts, 166 provider passed; five opt-in live tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 401 passed: conformance 23, durable 116, process 52, subagent 53, tools 157 |
| test-support/fake-provider | `uv sync --frozen`; `uv run --frozen pytest` | Pass; 51 tests |
| protocol/app-server | `pnpm install --frozen-lockfile`; `pnpm check`; `pnpm typecheck` | Pass; generated v4 drift/type checks |
| web-console | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm test` | Pass; 155 tests in 14 files |
| web-console | `pnpm check:provenance`; `pnpm build` | Pass; 56 source records, 100 production package notices |
| web-console | `pnpm test:e2e` | Six real App Server/browser tests passed |
| tui | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass; 812 tests, 96 suites, no skips |

The complete Rust unit run was repeated after adding preparation identity
reservation. TUI/browser tests used the rebuilt binaries; the full Web checks and
browser suite were repeated after the later inline selection correction. The
in-crate managed-package suite passed all 226 tests in 122.27 seconds; external
Tool tests passed all 157 in 97.62 seconds. Remote CI status belongs to the pushed
commit, not these local results.

## Negative audit

Source searches found no obsolete user upload method/types, old
`SessionUploadOwner`, or `text_only_editor_content`; no Context import of Local
Runtime/catalog persistence; no upload XML renderer in provider adapters; and no
ArtifactStore/ArtifactId in the Session upload owner or core upload projection.
`UploadedFileRef` still contains only `batch_id` and `name`. Generated v4 editor
content contains server receipt input rather than caller-authored canonical blocks.

Diffs against the reviewed head for `src/tools/artifacts.rs`,
`src/tools/background.rs`, `src/tools/executor.rs`, and
`src/local_runtime/session/deletion.rs` are empty. Tool runtime's only correction
is the core resolver field type. Managed spill/background behavior, `artifact/read`,
and frozen `DeletionRecord.upload_workspaces` are unchanged. No ignore-file edits,
caller-authored cleanup paths, auto-merge, or replacement PR were introduced.
