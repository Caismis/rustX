# Issue #319 validation

## Repository and scope

Implementation began from fetched `origin/main`
`e256998fda8d0fa56c47b8623e2c97d2eca46836`, including #306 / PR #318.
The original checkout was clean on `main` at that commit and remained unchanged.
Work ran in `/home/caismis/Documents/codes/rustX-issue-319` on
`issue-319-session-workspace-uploads`. Issue #319 and #305's superseding note,
the native path handlers, Session publication/deletion boundaries, protocol,
providers, Tool managed outputs, clients and CI were inspected before implementation.

See [the upload architecture](session-uploads.md) for the commit point, ownership,
wire bounds and schema boundaries. This is a replacement contract without an old
user-upload compatibility path.

## Deterministic acceptance evidence

`src/local_runtime/session/uploads/tests.rs` exercises the concrete owner:

- Exact workspace layout, ordered batches, restart resolution and concurrent
  same-name uploads with distinct exclusive allocations.
- Rejected path-shaped/reserved basenames, symlink ancestors, exclusive batch
  collision/no overwrite, descriptor-relative cleanup that preserves link targets,
  and rejected nested upload workspaces to keep Session cleanup roots disjoint.
- A Gate before readiness: complete materialized bytes remain unreceiptable until
  the durable ready commit; cross-Session receipts fail and upload alone creates
  no User message.
- A Gate that swaps an ancestor before readiness: failure remains durably owned,
  grants no receipt and writes nothing outside the allocation.
- Both independent fork and clone: the exact cut excludes later referenced and
  abandoned uploads; current mutable bytes are copied before visibility; source
  deletion leaves the destination usable; missing referenced source prevents
  publication.
- A Gate that substitutes a copied file with a symlink before publication:
  destination visibility is refused, staged residue is removed and external/source
  files remain intact.
- Same-Session branch shares the unchanged registry without filesystem copies;
  abandoned private copy claims recover after restart.
- Cwd changes retain both historical workspace roots; deletion freezes them;
  forced cleanup failure retains that record and restart retries it exactly while
  preserving another Session's files.
- Context estimation equals estimation of the actual XML-rendered input, including
  the absolute paths, and exceeds reference-only estimation.

`src/model/uploads.rs` proves exact XML escaping/order, byte-preserved user body,
no-upload identity and unchanged canonical facts. App Server scripted tests prove
receipt-only admission, fabricated canonical metadata rejection, failed admission
without fabricated history, image-as-workspace-file input to a text-only provider,
no eager file-byte injection and separate frozen request-time paths. A deterministic
lost-waiter Gate proves connection loss does not cancel or replay the admitted
Session mutation.

Web tests exercise ordered native receipts, Send gating, carrier limits, uncertain
transport outcomes and reconnect without replay. Browser acceptance drives a real
App Server and fake provider: uploads, typed history after reconnect, and retained
Tool artifact decoding/lightbox behavior. TUI tests render typed upload basenames
in order without reading model XML.

Existing managed artifact capacity, native spill, background, process, filesystem,
subagent and provider-emulator suites remain part of validation. No sleeps were
added as evidence for upload ordering or commit/publication correctness.

## Commands

The final branch's `.github/workflows/ci.yml` is the source of truth. Local Linux:
Rust/Cargo 1.95.0, Node 24.20.0, pnpm 11.13.1. The following checks were run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
git diff --cached --check
cargo build --bins
cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features
cargo test --lib --all-features -- boundary_suites::
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features \
  --test durable --test process --test subagent --test tools --test conformance

# test-support/fake-provider
uv sync --frozen
uv run --frozen pytest

# protocol/app-server
pnpm install --frozen-lockfile
pnpm check
pnpm typecheck

# web-console
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm check:provenance
pnpm build
pnpm test:e2e

# tui
pnpm install --frozen-lockfile
pnpm typecheck
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test
```

Results: Rust contracts **2,845 passed / one existing ignored**; external pure
contracts **25 passed**; providers **166 passed / five opt-in live tests ignored**;
in-crate boundaries **226 passed**. External boundaries: conformance **23**,
durable **116**, process **52**, subagent **53**, tools **157**, all passed.
Fake-provider pytest **51 passed**. Web **151 tests / 14 files**, provenance
**56 source records**, production build and **six browser tests** passed. TUI
**811 tests / 96 suites**, no skips. Generated protocol drift/type checks passed.

The existing production bundle-size advisory is non-fatal. Two repeat in-crate
boundary runs hit `source preparation liveness guard: Elapsed(())` at
`tests/boundary/managed_selection.rs:97` (225 passed / one failed each), with
`uv` still preparing real managed packages. The unchanged isolated test passed
in 142.65 seconds, and the final full suite passed all 226 in 129.62 seconds.
That last rerun was launched as `UV_OFFLINE=1 cargo test --lib --all-features --
boundary_suites::`; inspection of the actual `uv` child environments showed the
fixture discards that variable, so normal network preparation still ran. No
liveness guard, fixture or mandatory check was weakened or skipped.

macOS-specific execution is delegated to the PR's macOS CI runner; it cannot be
executed in this Linux workspace.
Browser screenshots remain in `web-console/test-results/` as local QA artifacts,
with CI configured to upload its own evidence.

## Negative audit

The final occurrence audit classifies retained artifact references as Tool-generated
managed output, its protocol/presentation/tests, or unrelated Markdown syntax.
The obsolete user carrier vocabulary has no remaining source/documentation matches.

```sh
rg -n 'artifact/upload|artifact_uploaded|ArtifactUpload|ArtifactUploaded' --glob '!issue-319-validation.md' .
rg -n 'rustx.app-server.v3|v3.schema|v3.ts' --glob '!issue-319-validation.md' .
rg -n 'user_uploaded_files' src/model/adapter
rg -n 'ArtifactStore|ArtifactId|artifact_id' src/local_runtime/session/uploads.rs src/model/uploads.rs
```

Each search returns no matches. `UploadedFileRef` contains only `batch_id` and
`name`; no absolute source path, ArtifactId or provider identifier is canonical.
The sole XML implementation is `model::uploads`; canonical history is unchanged
by rendering. Web uploaded-file cards use typed names, while `ArtifactResources`
and `artifact/read` serve managed Tool outputs. Background/executor/managed-spill
implementations are unchanged. The only ArtifactStore code change restricts the
obsolete public byte writer to a test fixture helper; native Tool writers remain.

Deletion takes only validated runtime Session IDs and catalog-authored workspace
allocations, never a caller cleanup path. Fork destination registry roots belong
to the destination, and the source-deletion test proves that independence. No
ignore file, migration, feature flag, alternate storage backend or upload rollback
transaction was introduced.
