# PR #422 review repair

Repair starts at `1f7c2788db7156f39cf2a09328bf185ce67206d9` on the existing
`issue-419-session-startup-ownership` branch/worktree. Fetched `origin/main`
remains `6ff49deb4c0855e64f0acaed85ebea2b6a277103`; no rebase or stacking.
The original worktree remains untouched, including `.playwright-mcp/`.

## Owners and linearization

`AppServerClient.receive` validates/correlates the response before removing the
pending entry and clearing its timeout. It invokes the acknowledgement evidence
observer at that decode boundary. Observer exceptions become console diagnostics,
not RPC errors. Response presentation is likewise separate from settlement:
a `finally` settles exactly once from the wire result/error and pumps the queue.
Malformed/mismatched responses still close the connection with pending mutations
intact for uncertainty classification. Neither observer failure nor duplicate
response delivery resends a mutation.

`SessionRuntimeManager.create_session` captures the full admitted configuration
once and passes it to `SessionController.create_session_with_binding`. The
controller prepares storage privately, acquires catalog and binding exclusion,
installs the exact binding, then publishes. Configuration coordination cannot
inspect or mutate the provisional binding before that visibility decision. There is no await from binding installation through
publication and pre-visibility rollback. Attach must acquire that catalog to
resolve identity; it cannot observe a published identity without its binding.
The manager registers its application scope from the same capture after publication;
it never replaces the binding after another client can attach. No runtime or
provider readiness is added to creation. A pre-visibility error removes the
provisional binding; a committed durability diagnostic retains it and the real
identity. Cancellation during private preparation retains the existing allocator
ownership; after binding installation the synchronous publication/rollback segment
cannot be abandoned by cancelling the async waiter. Cold-load capture still serves
Sessions reopened after process restart, whose process binding does not exist;
it cannot win a second capture for an identity currently being created.

`SessionCatalog.commit_under_ownership` compares old/new listable Session keys
and records membership invalidation only after a visible persistence outcome.
This is the common owner for `publish_session` (new/copy), `commit_planned`
(startup plan creating an identity, including first publication of an unpublished catalog), and `commit_delete` (removal into the frozen
deletion workset). Branch/current-node changes do not change membership.
`finish_delete` removes cleanup records, and `recover_delete` republishes frozen
cleanup authority: neither adds/removes listable membership. They therefore emit
no duplicate membership transition. Visible durability uncertainty announces the
transition; pre-visibility failure does not. Cleanup pending is already removed
membership, regardless of physical cleanup progress.

v24 remains mandatory. The final notification is
`session/summaryInvalidated { session_id, catalog_changed }`: false invalidates
only exact summary/display metadata; true invalidates catalog membership and the
named summary. It contains evidence, never replacement catalog/summary values.
Web's existing catalog owner rereads native authority independently of attach.
TUI consumes the unchanged v24 shape. Generated descriptions change, not wire types.

## Preserved scope and lifetime

Create ACK still commits navigation after first-input ownership transfer.
`FirstSubmissions` remains client-owned; remount observes rather than dispatches.
Endpoint/generation/authority/attachment fences, uncertain-response no-replay,
explicit/preference/omitted model intent, later model switching and frozen Attempt
authority are unchanged. There is no initial setModel/repair or blocking catalog
read. EN/ZH presentation and pinned Harness provenance are unchanged.
Terminal records remain small client-lifetime tombstones: admission/discard releases
text/Files/receipts, while the record preserves remount/draft-consumption semantics.
No timer/cache framework or new cleanup policy is added for this non-blocker.


## Deterministic regression mapping

- `ACK observer exceptions cannot strand a mutation or block queued request dispatch`:
  real Web client, one held create plus seven held reads fill capacity; a throwing
  evidence observer cannot prevent create resolution or dispatch of the eighth
  read. Duplicate response delivery does not repeat observation or mutation.
- `issue422_creation_binding_precedes_multi_client_visibility`: private preparation
  gate after capture A; an existing Session drives native source publication B;
  a fresh capture is asserted B. Creator is then parked after visibility while
  another initialized App Server connection receives membership invalidation,
  lists and attaches the new identity. Retained binding and live runtime use A's
  instructions and model; creator ACK names that same identity. Runs both durable
  success and injected post-rename durability uncertainty.
- `issue422_creation_previsibility_failure_removes_provisional_binding`: injected
  pre-rename failure leaves no listable identity, retained binding or invalidation.
- `issue422_multi_client_deletion_membership_converges_at_commit`: A lists, B
  deletes, A receives membership invalidation and rereads an empty catalog. Runs
  normal success and post-rename durability uncertainty through real App Server
  handlers.
- `issue422_deletion_membership_changes_once_before_cleanup_even_when_uncertain`:
  native membership disappears and is announced before cleanup; injected cleanup
  failure, recovery and final cleanup produce no second membership transition.
- Existing `issue419_*` native tests retain create invalidation, explicit/omitted
  model persistence, current default selection, first Attempt freeze and later
  intentional switching. Existing client/first-submit/new-conversation/residency
  suites retain uncertainty, fencing, receipt order, no replay and catalog
  independence. Production App browser startup gates/remounts exercise both locales.

Channels, scoped gates, acknowledged source application and held RPC responses
prove ordering; no sleeps establish a race. This repair does not add latency
measurements or make a startup/provider speed claim. The original before/after
measurements remain historical evidence for #419's navigation change.

## Final validation

Environment: Node 24.21.0, pnpm 11.13.1, rustc 1.95.0. Re-ran
`pnpm install --frozen-lockfile` in protocol/app-server, tui and web-console and
`uv sync --frozen` in test-support/fake-provider; all were already current.
No lockfiles or dependencies changed. Commands below ran in the implementation
worktree; Cargo build jobs were bounded to two and test threads to four.

| Executed command | Final result |
| --- | --- |
| `CARGO_BUILD_JOBS=2 cargo test --lib --all-features issue422 -- --test-threads=4` | 4 passed |
| `CARGO_BUILD_JOBS=2 cargo test --lib --all-features issue419 -- --test-threads=4` | 5 passed; opt-in measurement ignored |
| `CARGO_BUILD_JOBS=2 cargo test --lib --all-features a_repair_parked_past_deletion_neither_resurrects_nor_announces` | 1 passed |
| `pnpm --dir web-console exec vitest run test/client.test.ts test/first-submit.test.ts test/new-conversation.test.tsx test/conversation-residency.test.tsx` | 85 passed |
| `(cd web-console && CONTAINER_ENGINE=podman bash scripts/browser-tests.sh startup-ownership.spec.ts)` | 13 passed |
| `cargo fmt --all -- --check` | Pass, including final rerun |
| `CARGO_BUILD_JOBS=2 cargo clippy --all-targets --all-features -- -D warnings` | Pass, including final rerun |
| `CARGO_BUILD_JOBS=2 cargo build --bins` | Pass; real transport/browser lanes use these binaries |
| `CARGO_BUILD_JOBS=2 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features -- --test-threads=4` | 3,949 passed, 8 explicit ignores, 18 targets |
| `(cd protocol/app-server && CARGO_BUILD_JOBS=2 pnpm generate)` | Pass; only v24 schema/TypeScript descriptions changed |
| `(cd protocol/app-server && CARGO_BUILD_JOBS=2 pnpm check && pnpm typecheck)` | Pass after committing generated outputs; regeneration has no drift |
| `(cd tui && pnpm typecheck && RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test)` | Pass; 854 tests, including real stdio/WebSocket transport |
| `(cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance)` | Pass; 1,061 tests / 58 files, 143 provenance records, 131 package notices |
| `(cd web-console && CONTAINER_ENGINE=podman RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test:e2e)` | All 124 passed, including real-process/transport lanes |
| `git diff --check` | Pass |

The all-target Rust command includes in-crate App Server/boundary conformance,
3,338 library tests, cfg3_catalog (26), cfg3_managed_output (5), conformance (22),
contracts (28), durable (129), process (62), provider (166), subagent (43) and
tools (130). No cancellation, physical settlement or provider-isolation test
was weakened. The eight ignores remain the three opt-in library measurement/
fixture writers and five credential-dependent live provider tests. External
credentialed providers were not exercised; required lanes use isolated fixtures.
No new before/after latency comparison was performed for this repair.

The initial full Rust run exposed the obsolete assertion in
`a_repair_parked_past_deletion_neither_resurrects_nor_announces`: it expected no
invalidation even from deletion. The test now requires exactly one membership
invalidation at deletion and zero additional invalidations from the resumed
repair. Its focused rerun and the complete Rust rerun pass. New test construction
also corrected explicit fixture response types and established real source B
publication through an existing Session before asserting the creation race.

The first browser launch failed before tests because unrelated processes occupied
5173/5174. Both successful browser lanes used temporary 15173/15174 substitutions;
every port-only change was restored afterward. No unrelated process was stopped.
The pinned Playwright container/image and screenshot references are unchanged.
Inspected English/Chinese attaching and attachment-failure screenshots from the
actual App tests: native identity is visible with retained text/Files, input is
explicitly unsent during preparation, and failure retains the draft without a
replay action. Existing Vite large-chunk advisories remain non-failing.

The implementation worktree remains available for review. The original worktree
was neither edited nor switched. Delivery updates only the existing PR #422;
auto-merge remains disabled and CI is not watched after pushing.
