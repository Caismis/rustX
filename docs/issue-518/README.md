# Issue #518 — automatic Session configuration commit

Base: `origin/main` at `b0aa75fe110ca53a1dfc0637280f2f08508c8b7e`.
References (read-only, outside rustX): DeepSeek Harness
`ce7073d51254c26cd63b22a60835b8f948deb447`, Pi
`9c9ce8547e5a88cbe834a53e9a6d83a789d08e47`, Codex
`c60cda34a3ba6975f698d63da9e8ca8a1616f0c9`. The source-to-rustX map is in
[web-console/PROVENANCE.md](../../web-console/PROVENANCE.md#issue-518--automatic-session-configuration-commit).

## Native owners inspected

`local_runtime::configuration::application` (per-scope desired application,
units, worker), `session_runtime_manager::configuration` (source capture and
reconciliation), `runtime::conversation_runtime::{admission, configuration}`
(coordinator lock, worker loop, `admit_next_attempt`), the capability commit
grant (`try_commit_runtime` / `commit_available`), `durable::{store, sqlite}`
journal and transcript, `runtime_client::{projection, trace}`, and the App
Server protocol/connection layer.

## Rules

- **Staging.** Preparation stays off-side and bounded (one active preparation,
  latest desired work per scope). A complete candidate — context binding,
  capability closure, Skills, model state and approval mode — is staged in the
  resident runtime; a newer staged candidate replaces it (`Superseded`).
- **Commit point.** The staged candidate commits under the coordinator lock
  that linearizes Attempt admission: in the worker loop after runtime catalog
  publication, and in `admit_next_attempt` before batch selection. A ready
  candidate therefore always commits before the next independent Attempt
  captures.
- **Safe boundary.** Blocks only on a current or scheduled Attempt, manual
  compaction, a manual tail, a durability failure, or background/Subagent work
  that still holds the old configuration. Pending inbox, Goal continuation
  authority and recovered continuation do not block; the commit lands before
  their next Attempt. Residency/idle rules are unchanged.
- **Fences.** Each scope's desired attempt is an `Arc<AtomicU64>` epoch shared
  with the staged candidate; a superseded V2 can never commit after V3. A
  candidate whose baseline binding or owner moved is superseded and prepared
  again. Model selection keeps its own fence (`model_selection_conflict`).
- **Busy capability grant.** Keeps the candidate staged, defers admission and
  wakes on `commit_available()` outside the lock.
- **Frozen.** An admitted Attempt and its descendants keep their snapshot,
  fencing and leases; uncommitted capability candidates retire through their
  own owner.
- **Evidence.** Each binding change appends one `RuntimeEvent::ConfigurationApplied
  { change }` with its own transcript position (no Attempt/Turn), released in
  Journal order to the Runtime Client (`configuration_applied`), the transcript
  read domain and Trace (`TraceKind::Configuration`). No effective change, no
  fact. A failed append is an absorbing `record_configuration_change`
  durability failure and reports the commit `uncertain`.
- **External edits.** A bounded `notify` watcher routes changed paths to their
  owning source target and feeds `observe_source_change`, the same capture,
  fence and validation path as a native write. Coverage: User document
  directory (that file), `~/rustx/.agents` (recursive), each Workspace with
  source authority or a Session scope (`rustx.toml`, root `*.md`) and its
  `.agents` (recursive).

## Removed

`session/adoptConfiguration`, the `configuration_adoption` error, the available
candidate on `ConfigurationApplication`, Runtime Client adoption eligibility
(snapshot field and event), cache-impact consent (`model::request_shape`,
adapter `require_history`), `AdoptionEligibility`/`AdoptionError`/`IdleProbe`,
Web Adopt control and adoption actor region, the request-diff `SystemChanges`
Chat surface, TUI `/session adopt`, and every test/doc asserting a click.
Versions: App Server 63, Runtime Client 80, SQLite 83.

## Invariant → test

| Invariant | Test |
| --- | --- |
| No client drives the commit; every resident Session commits | `automatic_commit::t04_every_resident_session_commits_and_t09_policy_and_healthy_rescan_are_noops` |
| Admitted Attempt keeps its capture; next independent Attempt uses the successor | `automatic_commit::t01_new_attempt_captures_automatic_policy_old_attempt_retains_capture`, `issue383_disabled_extension_keeps_admitted_configuration_until_settlement` |
| Commit linearizes before a racing (queued) independent admission | `automatic_commit::t11_ready_candidate_commits_before_racing_independent_admission` |
| Real background owner defers; its settlement alone commits | `automatic_commit::t11_background_ownership_defers_commit_until_it_settles` |
| Subagent owner defers; settlement commits exactly once; stale baseline superseded; no fact without change | `configuration::tests::subagent_settlement_commits_the_current_staged_configuration` |
| V2 superseded by V3; failed latest never revives V2 | `automatic_commit::t11_newer_desire_supersedes_a_staged_candidate_and_rescan_does_not_reprepare`, `t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate`, `configuration_commit::tests::a_superseded_desire_is_never_current_again` |
| Explicit model choice beats a staged candidate | `automatic_commit::live_model_selection_supersedes_a_candidate_blocked_by_background_ownership` |
| Unload/load keeps the committed binding without replay | `automatic_commit::t15_unload_load_retains_the_committed_configuration_without_replay` |
| Change fact is a Session-level transcript row; failed append leaves nothing | `configuration::tests::configuration_change_fact_is_a_complete_transcript_row` |
| Same-name change is `changed`; bounded names with exact totals | `configuration_commit::tests::same_name_definition_change_is_changed_not_removed_and_added`, `name_lists_are_bounded_with_exact_totals` |
| External edit → same native path, bounded, coalesced; project files ignored | `automatic_commit::external_workspace_edit_commits_through_the_bounded_source_watch`, `source_watch::tests::*` |
| Web Chat hidden rows; single/multiple Tool rows | `web-console/test/configuration-change-row.test.tsx`; real server `test/e2e/trajectory-integration.spec.ts` |
| Web banner: preparing/waiting/failed/uncertain/restart; silent success; no rereads on stream | `web-console/test/session-configuration.test.tsx`, `test/e2e/settings-presentation.spec.ts` |
| Real App Server: busy boundary, automatic commit, frozen historical Trace | `test/e2e/trajectory-integration.spec.ts`, `test/e2e/settings-ownership.spec.ts` |
| TUI notice from live event and paged row; footer status only while pending | `tui/test/configuration-notice.test.ts`, `tui/test/status.test.ts` |

## Cost

One Journal row and one transcript reference per actual commit, nothing per
file notification, preparation tick or stream delta. Encoded change fact:
330 bytes for the browser scenario (instructions + one removed Tool); at most
6,862 bytes with every domain and six full 16-name lists of 64-byte names,
below the 16,384-byte header bound. No prompt or schema is copied; Trace and
transcript reuse the existing bounded reads. Streaming 300 deltas performs no
configuration read (`session-configuration.test.tsx` C18).

## Evidence

Real App Server + controlled provider (pinned Playwright container):

- [before: Adopt banner](evidence/before-adopt-banner.png)
- [waiting for current work](evidence/rustx-configuration-waiting.png)
- [Chat: one committed Tool removal, no prompt row](evidence/rustx-configuration-chat-desktop.png),
  [narrow](evidence/rustx-configuration-chat-mobile.png)
- [failed, previous kept, owner actions and Retry](evidence/session-banner-failed-light.png)

TUI notice (deterministic render, `tui/test/configuration-notice.test.ts`):

```text
• Configuration applied · instructions, tools
  Tools added: fetch, search +16 more
  Tools removed: read
```

## Limitations

- The uncertain path is proven at the store boundary (a failed append records
  nothing) and by the absorbing durability failure it maps to; no test injects
  that fault through a live commit.
- Regenerating `incremental-native.json` from current native code exposes a
  pre-existing gap unrelated to this issue: the host-materialized
  `auxiliary.audit` frontier is absent from `projection_transition` metadata.
  The fixture therefore keeps its base capture with only the removed
  eligibility vocabulary stripped.
- `stream_progress::large_write_progress_crosses_idle_timeout_without_retry_or_duplicate_execution`
  is load-sensitive: it failed under the full parallel suite and passes alone.
