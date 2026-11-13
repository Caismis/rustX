# Issue #532 implementation evidence

Implementation base: `4ecd28b8834d91566404ccc24a601ba7cf2e9b7d`.
Pinned Codex release: `rust-v0.166.0`, commit
`6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`.
The [method and native-owner matrix](shared-control.md) describes the public
contract, identity mapping, admission and settlement boundaries, deployment
responsibilities, and intentional exclusions.

## Invariants and executable evidence

| Invariant or boundary | Executable evidence |
| --- | --- |
| Capability file/hash and controlled-clock HS256 verification fail closed | `src/app_server/transport/auth.rs` tests; `tests/scripted/app_server/authentication.rs`; private credential and listener argument tests |
| Browser and native credentials use the same validator and ordinary API eligibility; JWT expiry affects new upgrades | `native_and_browser_share_validation_and_jwt_expiry_only_affects_new_admission`; standalone example uses a credential containing `/`, `+` and `=` |
| Three writers share one native Attempt; start-or-steer differs from queued input; stale controls cannot hit a successor | `shared_start_steer_sparse_queue_stale_controls_and_disconnect_use_one_native_writer` in `tests/conformance/app_server_host.rs` |
| One native mailbox retains sparse queue/steer identities and acceptance correlation | Native durable adoption tests; the shared-control provider conformance; Web and TUI lost-receipt tests |
| First correlatable result or RPC error claims once, including invalid domain results | `three_clients_broadcast_first_response_error_and_invalid_payload_commit_once`; `src/app_server/callbacks.rs` arbitration tests |
| Broadcast may execute two external effects but commits one canonical result and continuation | Real HTTP provider conformance records two application effects before either response and verifies one native result |
| A canceled or retired actor cannot publish a queued callback or claim a reply | Callback carrier-acceptance and cancellation tests; `retired_native_incarnation_is_fenced_before_closed_delivery`; actual catalog archive/unarchive regression |
| Every live request at the 128-request bound can replay before a reader runs | `reconnect_replays_the_entire_pending_bound_before_the_reader_runs`; bounded client queue and slow-consumer tests |
| Exact Thread history and catalog identity survive rename, archive, unarchive and restart | `thread_identity_tools_names_archive_and_unarchive_remain_native_metadata`; existing node-switch, deletion and residency suites |
| Host declarations validate namespaces, schemas, reserved names and frozen capabilities | `src/tools/host.rs` tests, root capability resolution, manifest serialization and real provider conformance |
| Process death never automatically repeats an uncertain external effect | `host_effect_is_unknown_after_process_death_and_is_never_automatically_replayed` in `tests/process/app_server_host.rs`: actual App Server, fsynced effect, SIGKILL, same-root reopen |
| Existing clients preserve prompts, drafts, exact targets, queued-message operations and lost-receipt repair | Web deterministic and browser suites; TUI state/controller and actual stdio/WebSocket integration suites |
| An independent external client needs no rustX SDK | `tui/test/external-example.test.ts` launches the real binary and runs `examples/app-server/connect.mjs` |

Composed conformance uses `test-support/fake-provider` through the actual catalog,
adapter, HTTP client and parser. Gates and native validity seams establish order;
timeouts bound failure. No paid model credentials are required.

## Local checks

Environment: Linux x86_64, Rust 1.98.1, Node 24.19.0, locked pnpm 11.13.1 and the
repository's Python/uv provider emulator. Browser references use the repository's
immutable Playwright 1.63.0 container. Results below refer to the integrated base
above and the completed implementation, including the independent review fixes.
The full-suite snapshot precedes the final interaction-reply numeric-codec
correction. Its follow-up App Server unit suite passed all 58 tests; final-head
FULL CI below owns the complete result after that correction.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins --all-features` | Pass |
| Provider emulator `uv run --frozen pytest` | 55 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --no-fail-fast` | Pass; Cargo reports 5999 passed, 20 ignored; includes actual process death and required-emulator conformance |
| `cargo run --all-features --example check_test_lanes` | Pass; Linux FULL discovery has no missing, empty or stale lane selections |
| `cargo test --all-features app_server:: --lib` after the reply-codec correction | 58 passed; exact Workflow u64 identities round-trip, noncanonical/overflow values fail |
| Protocol generation drift, editor schema generation and TypeScript checks | Pass; generated editor schemas have no drift |
| Development launcher typecheck and tests | Pass; 43 tests |
| TUI typecheck and required-emulator full suite | Pass; 1326 passed, 1 skipped |
| Web typecheck and full deterministic suite | Pass; 4181 passed, 4 skipped |
| Web production build, i18n, provenance and notices | Pass; 347 source records, 349 production package notices |
| Pinned-container browser acceptance | Full local traversal reported 633 passed and 34 failed. Focused repairs passed delivery 2/2, presentation 3/3 and all 60 Shell/navigation cases across two runs. The remaining review-codec repair's browser recheck awaited container startup at this snapshot. Its result and the complete final-head browser-shard/coverage verdict are recorded on [the primary PR](https://github.com/Caismis/rustX/pull/540) |
| Five-mode real-process performance measurement | Pass; 100 native Turns, exact 20/40 model requests per mode; [raw data and method](../../benchmarks/app-server/README.md) |
| `git diff --check` | Pass |

TUI's one skip is the native macOS ACL case; Web's four skips are native macOS
Office/Seatbelt cases. Required emulator cases ran. Native ignored live-provider
tests, measurements and fixture generators are not correctness coverage.
Fixture child entry points in Cargo's totals are not separate boundary proofs;
their parent tests own those claims.
Earlier partial or failed runs are not counted as
successful full-suite evidence. Final-head Linux, macOS and full browser CI must
all pass under `full-verification`; the [primary PR](https://github.com/Caismis/rustX/pull/540)
records the exact final head, run URL and full-suite verdict. This committed local
snapshot makes no claim about a later CI result. Local Linux checks do not
substitute for macOS execution.

## Independent review

A separate read-only review agent inspected the complete implementation and
re-reviewed it after integration with the latest main. It found seven P2 issues,
all fixed and re-reviewed: private credential bounds/truncation; encoded browser
bearer carriers; full-bound callback replay; resolved TUI pagination entries;
physical close after a wrong Thread receipt; retired native-incarnation fencing;
and ordinary display/edit/remove treatment for `queued_message`.

Follow-up source review also checked the browser wire probe's native Thread-to-
Session mapping and physical receipt barriers, the corrected ordinary-delivery
assertions, and the canonical integration documentation. The browser's real
history/reconnect test and the 21-test probe suite pass after the probe repair.
It also verified finite fake-server capabilities, current mutation guards and
the exact Attempt ID in interrupt assertions; 242 affected unit tests pass.
The final browser traversal also found a claimed review result bypassing the
existing exact-u64 wire decoder. App Server now reuses that decoder after claim;
an independent review verified that invalid results still settle once and that
free-form Host JSON is unchanged. Workflow completion assertions now check the
run header rather than accepting a completed child node.

The final independent source review found no remaining blocking issues. It also
checked native lifecycle/cancellation/broker lock order and the main branch's
submission-activity sampling. For the initial shared-control delivery, no WebUI/TUI controls, layouts, styles
or screenshot references were added, removed or redesigned. The v74 amendment
below updates the existing status surfaces and their corresponding references. Product changes adapt protocol,
identity and state synchronization behind the existing UI.
Static presentation fixtures declare only their implemented API subset. Their
unchanged references do not assert unchanged feature visibility against Native:
common ordinary API eligibility can make the existing side-question control
available on connections that previously lacked the extra delivery credential.

## Deliberate limits

This remains one user's trusted App Server, with admission-time JWT validation
and operator-owned credential distribution, TLS and browser Origin/CSP policy.
Broadcast does not guarantee one external effect. A remote error, disconnect,
cancellation after dispatch or process crash cannot prove that external work
stopped. The native Tool Runtime owns canonical result and terminal commitment.

The existing browser Delivery Access input remains visible to respect the UI
constraint. Ordinary API eligibility now comes from the connection credential;
the former extra delivery credential has no effect and is never sent. The
private Product Host root-proof channel retains its separate native ownership.

Command execution, new filesystem/management APIs and the other Issue B work
remain outside this change. The standalone connectivity example and its run
instructions are in [shared-control.md](shared-control.md#existing-clients-and-standalone-example).

### Review boundary regressions after `33c7f5f`

- `archive_after_identity_resolution_fences_the_registered_execution_owner` parks
  after the early catalog identity read and before Loading registration, commits
  archive, then exercises the production fresh-node composition admission. It
  proves zero compositions/model calls, preserves read-only access and permits
  explicit unarchive. `archived_cold_trigger_keeps_its_receipt_without_admitting_execution`
  applies the same rule to an already accepted cold scheduled trigger.
- `detach_revokes_only_the_departing_subscriber_without_leaking_capacity` repeats
  130 subscriber retirements while another subscriber retains native settlement
  authority. Native publication fences, same-channel retirement/replay order,
  shared TypeScript handler aborts and late-result suppression remain covered.
  Web's actual client repeats 130 attachment cycles without leaking callback slots.
- The rendered Web composer lifecycle tests deliver lookup acceptance to the
  original submission and First Submission ACK owner, including attachments,
  later edits and explicit native `outcome_unknown`, without input replay.
- TUI reconnect observation tests preserve the original operation record before
  close listeners, transfer it across Hosts, and query exact authority/Conversation/
  inputId. Accepted proofs settle it; absent or failed reads and mismatched
  authority/Conversation preserve uncertainty. Typed native unknown responses use
  the same path. None submits replacement input.

These targeted regressions do not replace final-head FULL verification for the
approved Thread runtime-status amendment.

## Thread runtime-status amendment (v78)

This delta preserves the four review-boundary fixes above and uses existing
Native owners: manager residency, Attempt/scheduled/manual physical task tails,
logical root/routed Interactions, Background/Child/Auxiliary execution, and
Native durability/MCP/recovery/retirement faults. The status projection grants no
control and adds no execution journal. Cold metadata stays separate from runtime
residency; nullable metadata proofs retain exact `thread_id` while strong Thread
queries return the existing typed `observation_unavailable` error.

| Amendment invariant | Deterministic evidence |
| --- | --- |
| Cold read/list/loaded enumeration never loads or calls a model; start, completion, unload, fresh incarnation and observer eviction | `thread_status_cold_reads_running_settlement_and_unload_need_no_attachment` |
| An absorbing Native fault cannot be cleared by reads; proven physical retirement and recomposition establish a healthy new incarnation | `thread_status_native_fault_is_absorbing_and_cannot_be_cleared_by_reads`; `thread_status_fault_crossing_native_retirement_never_reports_idle` |
| No false idle after the Attempt slot clears but its real physical owner remains | Native drain test's `activity().active` assertion at the existing Attempt exit gate |
| Full logical Interaction map, simultaneous approval/questionnaire flags, remaining approvals, fenced older cleanup | `thread_status_counts_logical_waits_beyond_the_observation_head_and_fences_old_cleanup` |
| Three subscribers share one Native wait; callback claim and resolved notification do not retire it before Native settlement | `headless_interactions_broadcast_survive_disconnect_and_select_one_response` with the before-settlement gate |
| Host application callback waiting is active without human flags; ordinary failures/unknown outcomes retain Native settlement | Public Thread read in `tests/conformance/app_server_host.rs`; existing Host process-death conformance |
| Unproven Child physical settlement and admission rollback are system faults instead of false idle or eternal running | `unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent`; `staged_rollback_uncertainty_survives_prepare_and_durable_admission_rollback` |
| Status acquisition cannot park Native admission/configuration; unavailable metadata stays readable and recovers even without an activity notification | Four real-lock barriers in `thread_status_busy_metadata_lock_recovers_without_a_semantic_wake` (Background, Coordinator, capture, composition); `live_catalog_publication_never_parks_on_background_ownership` |
| Meaningful-only publication, finite resident cache and explicit slow-consumer repair | `thread_status_publication_deduplicates_and_bounds_slow_consumer_delivery` |
| Early notifications, unknown exact summaries, repeated gaps, reconnect, per-ID failures and passive recovery | `web-console/test/thread-status.test.ts`; `tui/test/thread-status.test.ts` |
| Nullable proof revocation retains Native revision watermarks, including after cache eviction; late older/equal notifications cannot restore stale status | Shared bounded `ThreadStatusNotifications` exercised through both real clients' status tests |
| Controls, drafts, focus, stale-response fences and richer Native outcomes survive the projection change | Existing Web/TUI composer, attachment, Trace, recovery, actual-process and browser suites |

The independent read-only agent re-reviewed Native ownership/lock order,
publication and recovery, exact client identities and all revision-revocation
fixes. It found no remaining actionable source issues. The observer never waits
for a Native source while holding Registry, Coordinator or capture locks; its
separate recovery worker waits on each owner independently. A public unavailable
read wakes that worker even if the held operation emits no semantic notification.
Client proof revocation preserves finite revision evidence, without retaining
transcripts or introducing a second execution authority.

Final amendment validation is recorded below after all corresponding checks
complete. The old FULL run on `33c7f5f` does not verify v74; the primary PR must
record FULL verification for the amended final head, including Linux, macOS and
complete browser-shard coverage. Pending results are not counted as passes.

### Local amendment results

| Check | Result |
| --- | --- |
| Native Session runtime manager suite | 467 passed, 2 ignored |
| Native status, logical Interaction accounting, fault cut and four-source lock recovery | 6 passed |
| Protocol-version rejection over direct and stdio Runtime Client transports | 2 passed; historical v73 input remains independent of App Server v78 |
| Complete documentation/protocol contracts suite | 31 passed |
| Real App Server concurrent Sessions across external disconnect | Passed; instance status notices retain exact Thread identities and transcript isolation assertions |
| Child physical-proof and resume suites | 4 and 9 passed |
| `cargo fmt --all -- --check`; all-target/all-feature Clippy with warnings denied | Pass |
| Native bins; protocol regeneration with four artifact hashes unchanged; protocol TypeScript | Pass |
| Linux FULL test-lane coverage discovery | Pass; no uncovered runnable tests |
| TUI TypeScript and required-emulator full suite | Pass; 1342 passed, 1 platform skip |
| Web TypeScript and required-emulator full deterministic suite (`--maxWorkers=2`) | Pass; 4198 passed, 4 platform skips |
| Web production build, i18n, provenance and notices | Pass; 347 source records and 349 package notices |
| Pinned Playwright Shell, activity, composer and recovery acceptance | 60 passed, ordinary assertion mode after explicit reference review |
| `git diff --check` | Pass |

The 14 updated Shell references retain dimensions, controls and layout styles.
Pixel review confines differences to existing waiting-status copy, disconnected
status dots, and the former Queue/cancellation-intent status regions. Queue
acceptance is not runtime execution, and cancellation acknowledgement is not
Native settlement. The original generic waiting message is now the appropriate
Native approval/input label. Snapshot noise policies and comparison thresholds
were not relaxed.

Broad local monolithic Native runs are not counted as complete passes. They
exposed a historical Runtime Client fixture changed during the App Server version
bump, an outdated documented rejection boundary, and a process test that did not
yet recognize instance-scoped status notices. Each is corrected and its focused
follow-up passed, including the entire contracts suite. The same audit restored
an unrelated TypeScript icon path changed by the version replacement and
regenerated its provenance record. Under concurrent verification, existing
MCP/provider fixture gates also failed. The unchanged MCP one-round and repeated
ambiguous-HTTP cases passed their exact follow-up runs;
the complete manager and real TUI process suites also passed. Final amended-head
FULL CI, using the repository's separate verification lanes, remains the authority
for complete Native, macOS and browser coverage. Its exact head/run/result must
be recorded on the primary PR before this amendment is declared complete.

## Follow-up isolation and status-repair review

The review of `cb7608b6c5515072626e8393ddd1c2b15db001e4` closed the four
original P2 findings and identified three further boundaries. Their corrections
preserve existing Native admission, client controls and layout.

| Finding | Correction and deterministic evidence |
| --- | --- |
| Archive waited for one Coordinator while holding global Registry | Composition/idle probes run outside Registry. Reentry checks Session retirement, exact resident `Arc`, operation ownership and the native idle epoch before the nonblocking claim. `archive_idle_probe_never_holds_registry_while_another_thread_is_observed` holds A at both real boundaries while B's strong status and Session metadata reads complete. |
| Moving archive probes must retain admission fencing | `archive_rechecks_the_exact_resident_and_native_admission_epoch` covers an active competing Attempt, a fully released admission returning the same runtime to idle with a newer epoch, and replacement by a new resident. Existing archive/load and cold scheduled-trigger tests remain green. |
| A late failed status read revoked a newer recovery notice | Web/TUI capture the read's notification cut and connection/repair epoch. Failures revoke only starting evidence. Their status suites cover recovery before the failed response, delayed older notices, early-cache eviction, and positive/negative nullable responses. |
| Recovery evidence must survive proof replacement without resurrection | Weak receipt evidence follows retained UI objects and newer snapshots without ticking notification sequence. Exact revocation invalidates inherited aliases; reset discards them. Tests cover recovery20, snapshot30, then an older nullable response; an off-page Web view introducing a nullable catalog row; and revocation followed by cache eviction. |
| TUI catalog repair lost identities beyond metadata capacity | One bounded owner drains pending identities sequentially, waits for existing metadata capacity and rechecks work after waking. Tests cover all 32 cold rows crossing a gap, two unrelated occupied metadata slots, close during capacity wait, and eviction of the final queued interest. |

Final local source checks: Native manager **469 passed, 2 ignored**; Web
status/composer/interaction regressions **43 passed**; TUI status/reconnect/callback
regressions **28 passed**. Complete Web deterministic verification passed
**4202 tests, 4 platform skips**, and complete required-emulator TUI verification
passed **1347 tests, 1 platform skip**. Formatting, all-target/all-feature Clippy
with warnings denied, Native bins, Web production build, protocol/Web/TUI
TypeScript, provenance/notices and i18n pass.

An independent read-only agent re-reviewed all source corrections and their
additional cache/queue boundaries. Its separate actual TUI Host/FakeTransport
reproduction of recovery20, snapshot30 and a late nullable response now passes.
It found no remaining actionable source issues; its review does not substitute
for complete final-head CI.

The old-head FULL browser run `42371118714` found two more Agent waiting-header
references using the former generic label. Their explicit pinned-container
updates alter **554/528 pixels**, confined to the existing header at
`x439..516,y87..98`. Both and the Workflow editor case pass ordinary assertion
mode (**3 tests**). No UI component, style or global comparison threshold changed.

That run's stable Workflow graph capture differs at 746 curve pixels. Expanded
Playwright trace snapshots prove its complete `rf__wrapper` DOM byte-identical
to capture `42313212047`, at all three screenshot checkpoints; all five loaded
CSS resources are byte-identical. The recorded fourth raw PNG/trace/CSS hashes
and RGBA observations extend the existing policy only to the measured union:
**779 disjoint 1x1 sites**, each with its observed per-site maximum (largest 35).
The unchanged Workflow reference remains authoritative. The formerly negative
site `(749,306)` is now measured; its unobserved adjacent site `(749,305)` remains
an explicit rejection, as do other outside pixels and one level beyond each
bound. After these test-evidence additions, the comparator/stability suites pass
**81 tests**. The 4202-test Web result precedes only these evidence/test additions;
the full final-head CI must include them. The browser-internal rasterization
cause is not claimed to be established.

Run `42371118714` is failed checkpoint evidence, not final-head verification.
The primary PR must record a new complete FULL run for the final follow-up head
before checking off the amendment or declaring it ready.


## Admission refusal and attached-status retention follow-up

The review of `ea545822ac168264a393477e6ca57b08edfeed9d` confirmed the preceding
three corrections and identified two additional lifecycle boundaries.

| Finding | Correction and deterministic evidence |
| --- | --- |
| Archive admission refusal was retained as initialization failure | Native `ThreadArchived` classifies this refusal before composition while preserving fresh metadata under Loading ownership and the existing RPC diagnostic. Only this admission result is excluded from initialization failure retention; genuine MCP/preparation failures retain their retry path. The cold-trigger regression failed on the old code at immediate ordinary load after unarchive, then passed with one exact durable receipt, one `InboundTurnAdopted` message identity, one composition and one model request. No retry or manager storage operation intervenes. |
| Catalog eviction dropped an attached Thread's proof or pending read | TUI excludes live attached identities from the 128-entry catalog proof/interest eviction rules. Existing attachment lifetime bounds the protected identities; detach, Native retirement and node switching restore ordinary cache eligibility. Real Host/Session tests browse 160 distinct cold catalog identities while the original attachment stays live, then check detach/retirement and the exact catalog bound. Another regression holds both metadata slots while browsing and proves the attached pending read dispatches exactly once when capacity returns. |

Native manager verification passes **469 tests, 2 ignored**, including the genuine
failed-initialization receipt regression. The focused TUI status/reconnect/callback
suites pass **30 tests**. The complete required-emulator TUI suite passes
**1349 tests, 1 platform skip**. Pinned-pnpm TypeScript, formatting, all-target/all-feature
Clippy with warnings denied, actual Native bins and whitespace checks pass.
An independent read-only agent found no remaining actionable issues, independently
ran **51 TUI tests** spanning status, Session focus and branch switching, and
reviewed the Native failure-retention control flow without claiming a Rust run.
This follow-up changes no Web/TUI UI elements or public protocol DTOs.

Complete final-head FULL CI remains required on the primary PR before marking the
amendment complete or ready. Earlier-head runs do not verify these corrections.
