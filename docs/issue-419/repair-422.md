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
