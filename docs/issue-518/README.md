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
  capability closure, Skills, model state and execution policy — is staged in the
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
- **Busy capability grant.** Keeps the candidate staged and holds back the
  admission (independent or scheduled) that reached it; the worker wakes on
  `commit_available()` outside the lock, commits, and resumes exactly that
  admission with no further message.
- **Closed allocation.** A runtime that drained itself in place (failed
  physical reclamation) is not offered for preparation: its application defers
  to the next allocation like a cold Session, finitely. Attaching a client to
  the allocation that already holds a Ready candidate keeps it; only a
  different allocation re-prepares (`Reallocation`), and that rebind consumes
  any cold deferral, so a later attach to it keeps the new candidate. The
  staged marker names the application that staged it; only that
  application's settlement clears it.
- **Frozen.** An admitted Attempt and its descendants keep their snapshot,
  fencing and leases; uncommitted capability candidates retire through their
  own owner.
- **Evidence.** Each effective change appends one `RuntimeEvent::ConfigurationApplied
  { change }` with its own transcript position (no Attempt/Turn), released in
  Journal order to the Runtime Client (`configuration_applied`), the transcript
  read domain and Trace (`TraceKind::Configuration`, counted in the Trace
  total by the same `TraceFactKind::is_anchor` the queries use). A Tool
  compares by definition and implementation: its server binding, or the
  enabled Workflow program digest with the leaf Tools its direct Tool nodes
  call, or each Named Agent a Workflow froze or `subagent` admits — its
  definition digest, Tools, Skill versions, project instructions and the
  authorized environment it inherits. Every Tool source beneath it compares
  its whole captured binding, credentials included, prepared yet or not, so a
  source a child prepares on demand hides neither its own move nor the rest
  of the Agent; the catalog keeps only the transport-and-policy digest and the
  environment's variable names. A Skill compares by content version; an Agent or source
  nothing runs changes nothing. The Model domain compares the invocations that
  run (the Session's selections, each explicit Named Agent model and each
  direct Workflow Model node at any depth), over
  their provider binding, wire model, compat and parameters, never the client
  view or catalog entries nothing selects; preparation uses the same set, so
  such a change is never published as catalog-only, and a failed preparation
  never reports Provider `applied` for one it did not install. The whole
  effective Tool environment — authorized entries and the shared Skill
  Python/Node environments — is `execution_policy`, since every Root Tool
  runs in it. Preparation compares the binding of every source an admitted Agent
  selects too, so moving one is adopted.
  The independently
  published execution-policy unit (approval mode, model timeout, Tool
  deadline, MCP startup) records its own `execution_policy` fact (equal
  binding revisions). The same transaction retains the exact effective
  instructions (`ContentSource::ConfigurationInstructions`) and a JSON catalog
  of every added, changed or removed Tool and Skill with its `before` and
  `after` state (`ConfigurationCatalog`: definition, model schema,
  server-binding digest of transport and policy — never credentials —
  Workflow identity and Agent digests; Skill entry and version), read through
  `content_read` from the row (`instructions`, `catalog`) or the Trace record
  (`configuration_instructions`, `configuration_catalog`). Name lists are
  bounded by count (16) and encoded bytes (1 KiB each). No effective change, no
  fact. A failed append is an absorbing `record_configuration_change`
  durability failure and reports the commit `uncertain`; Web and TUI say it is
  published but not recorded. The attached allocation owns that fact apart
  from its durability gate, which keeps only its first failure: every
  application view reads it, so no unit reads `applied` while it holds,
  however late the settlement, whatever no-op edit follows, and whichever
  durable failure came first. Units keep their real outcome, so a published
  unit is still retained for reload; a healthy replacement allocation
  recovers the view and notifies its observers. Every entry that publishes a
  resident (a client load, a Scheduler cold start, recovery) attaches it at
  that one publication, before reporting it loaded, and only while it is
  still the Session's resident incarnation: a replaced allocation's late
  attach never takes the application back. The `.agents` resource revision
  counts only configuration entries, so Session uploads written beside them
  never conflict with a configuration read or capture.
- **External edits.** A bounded `notify` watcher routes changed paths to their
  owning source target and feeds `observe_source_change`, the same capture,
  fence and validation path as a native write. Coverage: User document
  directory (that file); `~/rustx/.agents` for `mcp.toml` and the loader's
  guidance files, with `agents`, `skills`, `tools` and `workflows`
  recursively; each Workspace with source authority or a Session scope
  (`rustx.toml`, root `*.md`, the guidance files) and its `.agents` alike. A
  notification at or above a watched directory re-registers its watches (a
  directory replaced at the same path), and only a watch that registered is
  active. A batch re-registers its watches before it captures, so an edit
  landing after that capture is always notified. Session uploads under
  `.agents/uploads` are never watched (the full browser suite caught an early
  version that did).

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
| Busy capability boundary defers admission; release commits and admits once with the successor, no new message | `automatic_commit::busy_capability_commit_defers_admission_until_the_boundary_releases` |
| A runtime drained in place defers finitely; other Sessions commit | `automatic_commit::a_runtime_closed_in_place_defers_its_application_and_others_still_commit` |
| Re-attaching the same allocation keeps its Ready candidate (one preparation, one Source fact) | `automatic_commit::reattaching_the_same_runtime_keeps_its_ready_candidate` |
| A reallocation consumes its cold deferral; a later attach to it keeps the candidate | `automatic_commit::a_reallocation_consumes_its_deferral_and_a_later_attach_keeps_the_candidate` |
| Skill body-only, Tool implementation-only and Workflow program-only changes record one `changed` fact; the next call runs the new program | `automatic_commit::body_only_skill_edit_records_one_changed_fact`, `mcp_readiness::implementation_only_tool_change_records_one_changed_fact`, `automatic_commit::workflow_program_only_change_records_one_changed_fact` |
| Execution-policy unit records its own fact without rebinding: approval, timeout alone, deadline alone; running Attempt keeps its capture | `automatic_commit::t04_every_resident_session_commits_and_t09_policy_and_healthy_rescan_are_noops`, `automatic_commit::timeout_and_deadline_edits_alone_each_record_one_execution_policy_fact` |
| Unused commits each keep readable instructions and affected-item catalog; wrong refs refused | `configuration::tests::configuration_change_fact_is_a_complete_transcript_row`; real server `test/e2e/trajectory-integration.spec.ts` (read before first use, Inspector) |
| Successive Skill versions with no request stay readable after the file moved on and the runtime is gone; Tool catalog carries a binding digest, no credentials | `automatic_commit::body_only_skill_edit_records_one_changed_fact`, `mcp_readiness::implementation_only_tool_change_records_one_changed_fact` |
| An unrecorded commit stays `uncertain` across a comment-only edit, also when its settlement arrives after that edit; admission fenced | `automatic_commit::an_unrecorded_live_commit_is_uncertain_and_fences_admission`, `automatic_commit::a_late_unrecorded_settlement_is_never_reported_applied` |
| A policy published after an unrecorded commit survives unload/load; the healthy replacement recovers the view and notifies its observers | `automatic_commit::a_published_policy_after_an_unrecorded_commit_survives_reload` |
| A commit left unrecorded after an earlier, unrelated durable failure stays `uncertain` across a comment-only edit | `automatic_commit::an_unrecorded_commit_behind_an_earlier_durability_failure_stays_uncertain` |
| An Agent edit changes exactly the Tools that run it (`review` Workflow, `subagent`); an unadmitted Agent changes nothing; the next call runs the edit | `automatic_commit::named_agent_edits_change_exactly_the_tools_that_run_them` |
| A new version of a Skill only a delegated child selects changes `subagent` once, with Root prompt and declarations unchanged; a Skill nothing runs changes nothing | `automatic_commit::a_child_only_skill_version_changes_the_tool_that_runs_it` |
| A child whose MCP source Root never prepares still records its Skill edit and its source move as `subagent` changes, and the move is adopted | `mcp_readiness::a_lazy_child_source_hides_neither_its_skills_nor_its_binding` |
| Moving the MCP source beneath a Workflow Agent node or direct Tool node changes that Workflow once; moving a source nothing runs changes nothing | `mcp_readiness::a_moved_source_beneath_a_workflow_agent_changes_the_workflow`, `mcp_readiness::a_moved_source_beneath_a_workflow_tool_node_changes_the_workflow` |
| A child-only model parameter change and a provider endpoint move under an unchanged Session view are each one Model fact; adding an unselected model records nothing | `automatic_commit::model_facts_follow_the_invocations_that_run` |
| A runtime only the Scheduler starts cold recovers the uncertain view and notifies observers | `schedule::workflow::a_scheduler_cold_start_recovers_the_configuration_application` |
| `load()` returns only after its runtime owns the application; parked before the attach, it has not returned | `automatic_commit::a_load_returns_only_after_its_runtime_owns_the_application` |
| A replaced allocation parked before the application lock never reattaches over its successor's unrecorded commit | `automatic_commit::a_replaced_allocation_never_reattaches_late` |
| A failed preparation of a Workflow-model and Workflow-prompt capture reports Provider failed and installs nothing | `automatic_commit::a_failed_preparation_never_reports_an_uninstalled_workflow_model` |
| A new shared Skill environment root is `execution_policy` with every authorized entry, Tool and Skill unchanged; no path is retained | `automatic_commit::a_new_skill_environment_is_execution_policy` |
| Session uploads and unread `.agents` entries leave the resource revision unchanged | `resource_directory::tests::only_configuration_entries_change_the_revision` |
| A parameter change of a model only a direct Workflow Model node names is one Model fact; an unselected model records nothing | `automatic_commit::a_workflow_model_node_is_a_model_that_runs` |
| An environment value change changes `subagent` and execution policy; the catalog names the variable, never its value | `automatic_commit::an_environment_value_change_is_recorded_without_its_value` |
| A credential reference switch on a source only a lazy child runs on changes `subagent`; no credential reaches the catalog | `mcp_readiness::a_hidden_source_credential_switch_changes_the_tool_that_runs_it` |
| A removal of 17 bootstrap Skills (no request, no earlier fact) keeps every removed definition, readable after the runtime and files are gone | `automatic_commit::a_removal_retains_every_removed_definition` |
| Oversized/escaped names stay within the header; Trace total counts configuration (alone and mixed) | `configuration_commit::tests::name_lists_are_bounded_by_encoded_bytes`, `trace::tests::header_tests::configuration_records_are_counted_and_unbounded_names_stay_readable` |
| Real background owner defers; its settlement alone commits | `automatic_commit::t11_background_ownership_defers_commit_until_it_settles` |
| Subagent owner defers; settlement commits exactly once; stale baseline superseded; no fact without change | `configuration::tests::subagent_settlement_commits_the_current_staged_configuration` |
| V2 superseded by V3; failed latest never revives V2 | `automatic_commit::t11_newer_desire_supersedes_a_staged_candidate_and_rescan_does_not_reprepare`, `t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate`, `configuration_commit::tests::a_superseded_desire_is_never_current_again` |
| Explicit model choice beats a staged candidate | `automatic_commit::live_model_selection_supersedes_a_candidate_blocked_by_background_ownership` |
| Unload/load keeps the committed binding without replay | `automatic_commit::t15_unload_load_retains_the_committed_configuration_without_replay` |
| Change fact is a Session-level transcript row; failed append leaves nothing | `configuration::tests::configuration_change_fact_is_a_complete_transcript_row` |
| Same-name change is `changed`; bounded names with exact totals | `configuration_commit::tests::same_name_definition_change_is_changed_not_removed_and_added`, `name_lists_are_bounded_with_exact_totals` |
| External edit → same native path, bounded, coalesced; project files ignored | `automatic_commit::external_workspace_edit_commits_through_the_bounded_source_watch`, `source_watch::tests::*` |
| Every loader guidance file is watched (User `AGENTS.md`, Workspace `AGENTS.MD`) | `automatic_commit::external_guidance_edits_commit_through_the_source_watch` |
| A replaced Skill directory is re-watched before capture; an edit right after that capture commits | `automatic_commit::a_replaced_skill_directory_keeps_feeding_the_source_watch`, `source_watch::tests::a_notification_reregisters_the_watches_at_or_below_it` |
| Web Chat hidden rows; single/multiple Tool rows | `web-console/test/configuration-change-row.test.tsx`; real server `test/e2e/trajectory-integration.spec.ts` |
| Web banner: preparing/waiting/failed/uncertain/restart; silent success; no rereads on stream | `web-console/test/session-configuration.test.tsx`, `test/e2e/settings-presentation.spec.ts` |
| Inspector content keeps one held read across parent rerenders; a new reader or cut retires it | `web-console/test/configuration-content-reader.test.tsx` |
| Real App Server: busy boundary, automatic commit, frozen historical Trace | `test/e2e/trajectory-integration.spec.ts`, `test/e2e/settings-ownership.spec.ts` |
| TUI notice from live event and paged row; footer status only while pending; uncertain never claims "previous kept" (wide and compact) | `tui/test/configuration-notice.test.ts`, `tui/test/status.test.ts` |

## Cost

One Journal row, one transcript reference, one retained copy of the
effective instructions and one JSON catalog of the affected Tools and Skills
(before and after states) per actual commit (unchanged items are not copied), nothing per file notification,
preparation tick or stream delta. Each name list is capped at 16 names and
1 KiB of encoded names whatever a source names its Tools, so a change fact
stays under 8 KiB (asserted), below the 16,384-byte header bound. The
instructions are stored once per commit in the existing content segments and
read in bounded 64 KiB ranges. Trace and transcript
reuse the existing bounded reads. Streaming 300 deltas performs no
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

- Regenerating `incremental-native.json` from current native code exposes a
  pre-existing gap unrelated to this issue: the host-materialized
  `auxiliary.audit` frontier is absent from `projection_transition` metadata.
  The fixture therefore keeps its base capture with only the removed
  eligibility vocabulary stripped.
- `stream_progress::large_write_progress_crosses_idle_timeout_without_retry_or_duplicate_execution`
  is load-sensitive: it failed in two earlier full parallel runs, passes alone,
  and passed in the final full run.
