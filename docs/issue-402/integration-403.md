# PR #404 integration with merged #403

Worktree: `/home/caismis/Documents/codes/rustX-issue-402`.
Branch: `issue-402-harness-conversation-models`.
Original implementation base: `da43450b77d3c195d95b06818be8142317663c91`.
Pre-rebase reviewed HEAD: `726d12cc8fcf507a6349ea17161c96358a41fb7b`.
Final integration base: `5df40f29c72ee5179864bf2f4671c3f8d32dbc18`.
Rebased implementation HEAD: `e16761c7676640b3c9e8d6badff0828e98cb3332`.

Both commits rebased without textual conflicts. `git range-diff` reports both
patches unchanged. This integration adds only this validation record and updates
the report's base description; it does not redesign either implementation.

## Semantic integration audit

The complete seven-commit main-side range was inspected, including the clap
grammar and exact-value/native-path corrections. The intersection is native
configuration source/application identity:

- `configuration.rs::source_manifest_revision` hashes a versioned, length-framed
  sequence of exact Unix path bytes and source revision bytes in BTreeMap order.
  Different native paths with equal lossy spellings remain distinct. JSON and
  presentation fields do not define native identity.
- `settings.rs::capture_source_settings` captures the User/Workspace document
  byte revisions (including absent/invalid documents) and applicable authored
  resource-tree revisions. It returns native input identity alongside the
  separately projected `SourceSettings`. Final document/resource rereads still
  reject changed captures. Source-write CAS still uses native document revisions.
- `SessionRuntimeManager::capture_source_consumers` uses that native identity,
  not serialized `SourceSettings`. The existing application coordinator retains
  no-op detection, generation fencing and worker publication. The source-write
  lock still transfers acknowledged writes to native coordination before reply.
- Product Host `configureWorkspace` resolves the registered Workspace, sends
  exact native CAS, and rereads `configuration/sourcesRead`. It keeps the commit
  acknowledgement separate from reread failure. The Settings target/unit actors
  consume these authoritative facts; `workspaceApprovalBlock` only reads those
  actors. Browser code neither hashes revisions nor derives source authority from
  a Session cwd.
- `composition.rs` resolves a new Session's persistent intent through
  `UserConfigManager::resolve_session`, which captures current canonical sources,
  resolves configuration/resources and admits them. Runtime Attempt admission
  copies `effective_approval_mode` into `CurrentAttempt.configuration`. Thus the
  existing post-commit authoritative source observation remains the required
  fence. Applying configuration to an unrelated resident Session is not a new
  prerequisite, and no browser polling or second transaction owner is needed.
- App Server validates lossless Unicode process bindings before bootstrap and
  again after canonical binding, before storage/readiness. Native CLI paths remain
  OS-native; diagnostic projection is fallible. #404 does not alter these files,
  normalize paths, or weaken the wire boundary.

`Cargo.toml` and `Cargo.lock` have zero delta from the integration base. The merged
clap 4.6.7 / clap_builder 4.6.7 / clap_derive 4.6.7 / clap_lex 1.1.1 closure remains
intact. No frontend dependency or Harness pin changed. The exact Harness pin is
`ddefc45fbc7f8e46dd73185e68295696d1297887`.

## Protocol and provenance

The single intentional transition remains App Server 20→21, Runtime Client 46→47
and SQLite 43→44. There is no v22, v20 compatibility import, dual schema or
migration. Current Rust constants, version assertions, generated schema/TypeScript,
fixtures and Web/TUI consumers agree. Historical provenance notes mentioning v20
describe the original #394 adaptation; current imports are v21.

Canonical generation/check and fixture typechecking passed with zero generated
diff. Provenance checks verified actual local hashes, the pinned upstream source,
the import boundary and dependency notices. No #403 native source is classified
as Harness-derived, and no notice or lockfile churn resulted from integration.

## Deterministic proof mapping

- `first-submit.test.ts` and `new-conversation.test.tsx`: known Host/RPC rejection
  preserves editable draft and permits only explicit retry; typed uncertain
  creation cannot replay; acknowledged Session identity survives authority
  replacement before attach/model/upload/send.
- `settings-machines.test.ts`: held write and unobserved commit block Send;
  conflict/uncertainty preserve corrective intent; authoritative reread releases
  the same existing target/unit transaction authority.
- `convergence.spec.ts`: hold permission write, then hold native acknowledgement
  and reread publication; disabled Send and Enter admit nothing in both stages.
  Releasing observation permits explicit submission, exactly one Session and turn,
  and native `AttemptStarted.execution_settings.approval_mode == full_access`.
- `turn-process.test.tsx`: early Status, independent User anchor, Status-only
  process, stable disclosure identity across pagination, distinct Attempts, and
  final reasoning/text separation retain native membership and anchors.
- `native_source_application_identity_uses_exact_facts_not_projection`: merged
  #403 source identity remains independent of JSON and mutable presentation,
  distinguishes exact paths and changes with authored bytes/resources.
- Full native tests retain the admitted-policy t01/t02 regressions: later source
  changes cannot rewrite an already-running Attempt's captured approval.

## Validation and browser evidence

Results recorded below are from the rebased tree, not earlier PR runs.

| Command (repository root unless stated) | Result |
| --- | --- |
| `git rebase origin/main` | Pass, no conflicts; both patches unchanged in range-diff |
| `git diff --check` | Pass |
| `pnpm --dir web-console install --frozen-lockfile` | Pass |
| `pnpm --dir protocol/app-server install --frozen-lockfile` | Pass |
| `pnpm --dir tui install --frozen-lockfile` | Pass |
| `pnpm --dir dev install --frozen-lockfile` | Pass |
| `pnpm --dir web-console exec vitest run test/first-submit.test.ts test/new-conversation.test.tsx test/settings-machines.test.ts test/turn-process.test.tsx` | 172 passed |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | 934 passed, 53 files |
| `pnpm --dir web-console check:provenance` | Pass, 135 source records and 131 production-package notices |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-402-harness` | Pass, exact pinned source |
| `pnpm --dir web-console build` | Pass, existing chunk-size advisory |
| `pnpm --dir dev typecheck` | Pass |
| `pnpm --dir dev test` | 37 passed |
| `pnpm --dir tui typecheck` | Pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 852 passed |
| `uv sync --frozen` in `test-support/fake-provider` | Pass |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `cargo fmt --all -- --check` | Pass |
| `CARGO_BUILD_JOBS=2 cargo test --lib --all-features --locked native_source_application_identity` | 1 passed |
| `CARGO_BUILD_JOBS=2 cargo build --bins --locked` | Pass |
| `CARGO_BUILD_JOBS=2 cargo +1.92 check --workspace --all-targets --all-features --locked --target-dir target/msrv` | Pass |
| `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets --all-features --locked` | Pass |
| `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh convergence.spec.ts` | 1 passed, 17.6 seconds |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 89 passed, 5.8 minutes |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 CARGO_BUILD_JOBS=2 cargo test --workspace --all-targets --all-features --locked` | 3,913 passed, zero failures, seven existing ignores |
| `CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Pass |
| `CARGO_BUILD_JOBS=2 pnpm --dir protocol/app-server check` | Pass, canonical regeneration with zero drift |
| `pnpm --dir protocol/app-server typecheck` | Pass |

No integration validation gate failed. One managed-MCP process-reuse test reported
running longer than 60 seconds and then passed under its existing behavior;
no timeout, test, assertion or implementation was changed. MSRV and all local
execution were on Linux; macOS remains a CI lane, not a claimed local result.
The complete diff and generated/provenance/dependency closure were reviewed before
delivery. A final fetch confirmed the integration base was still current.

Fresh real-browser captures under
`web-console/test-results/convergence-Harness-New-Co-4d712-cess-and-Models-convergence/`
were manually inspected at 1440×1000, dark theme:

- `05-permission-write-pending.png`: requested Full access is visible, draft is
  retained, Send is disabled, and the applying status is concise.
- `07-process-folded.png`: User remains outside, Status/process are folded,
  disclosure precedes the visible final answer.
- `08-process-expanded.png`: disclosure precedes anchored Agent Status, reasoning
  and Tools, with final textual answer outside the fold.

No visual regression or intended presentation change resulted from integration.
Live timestamps, temporary Workspace paths and duration labels differ between
runs. Existing committed evidence remains in `docs/issue-402/browser`; no expected
snapshot or committed screenshot was regenerated. Axe rules and pixel thresholds
were unchanged.

## Delivery transport recovery

Ordinary lease-protected delivery encountered stalled SSH (port 22), HTTP 408,
SSH-over-443 timeout, and a stalled HTTP/1.1 retry. A temporary server-side merge
into this PR branch produced tree `f68e5ef5fa257a277969667e380cc02866581d98`, exactly
the tested rebased implementation tree. It did not merge the PR into main. Even
with that tree present, Git resent a 3.5 MB pack and received HTTP 408 again.

The bounded recovery uses GitHub Git Data to publish the exact locally hashed
trees/commits, verifying each returned SHA, then GraphQL `updateRefs` with an
explicit `beforeOid` to preserve atomic expected-head protection for the
non-fast-forward branch replacement. GitHub documents this as an atomic update
that rejects a mismatching prior OID:
<https://docs.github.com/en/graphql/reference/git#updaterefs>.
No main ref or other issue branch participates. The final branch is the local
rebased history, not the temporary merge history; ordinary `git push
--force-with-lease` is also verified after synchronization. These are delivery
transport failures, not failed integration tests or changes to the validated code.
