# Full Web conformance gate — #313 / Epic #303

This is a map of executable evidence, not a new semantic specification. The
browser tests use one existing Playwright configuration and `startDogfood` fixture,
real App Server WebSockets, local provider HTTP/SSE and the actual Node Product
Host. Owner races stay in deterministic Rust tests; component tests cover bounded
presentation mechanics. Passing only the browser suite is insufficient.

## Prerequisite baseline

Fetched base: `9980fc719a573ece0cab0114311345dc4c78b621`. The following issues are
closed and their merge commits are ancestors of this base:

| Issue | Merged PR | Merge prefix |
| --- | --- | --- |
| #305 WEB-02 | #317 | `94dfa0ae` |
| #306 WEB-03 | #318 | `e256998f` |
| #307 WEB-04 | #321 | `5537d1e2` |
| #308 WEB-05 | #322 | `d5e56132` |
| #309 WEB-06 | #323 | `5e5359cd` |
| #310 WEB-07 | #324 | `e15b55e0` |
| #311 WEB-08 | #325 | `58f17e3c` |
| #312 WEB-09 | #326 | `9980fc71` |
| #319 WEB-02A | #320 | `289bd22e` |

WEB-01 #304/#316 (`8b8e99e8`) is also included. #319 is mandatory because of the
later acceptance dependency in #313's comment; the original dependency line alone
is incomplete. No compatibility path for pre-#319 chat artifacts is retained.

## Evidence by product boundary

Paths in the Browser column are relative to `test/e2e/`; component paths to `test/`.
Native paths are relative to the repository root. The full native commands in
VALIDATION.md execute these existing suites; their evidence is not inferred from
what a screenshot happens to show.

| Contract | Browser composition | Deterministic owner / presentation evidence |
| --- | --- | --- |
| Multi-Session and residency | `console.spec.ts`: A parked while B executes, TUI controller handoff, close/open beyond the 32-attachment capacity, disconnect/reload, explicit unload/cold reopen, durable identity/cwd/history, no duplicate side effect | `tests/scripted/app_server/{protocol,residency_policy,transports}.rs`: operation/idle/attach winners, manual-clock idle eviction, connection ownership; `client.test.ts` generation fencing and exact release |
| Chat / Markdown / history | `chat.spec.ts`: 34 durable turns cross the bootstrap page, prepend anchor geometry, streamed/settled rich text, reconnect, managed image decode/lightbox | `transcript.test.ts`: 512-entry/8-MiB finite window, stale pages and concurrent live refresh; Markdown incremental/code-settlement tests prove bounded current-document retention and canonical settlement; `scroll.test.tsx` measures stable anchors |
| Session-owned uploads | `uploads.spec.ts`: text + PNG, filesystem bytes, exact absolute model paths/XML-before-body, real native bash file IO, browser Fork, source delete before destination execution, reload, deletion of each owned root; no `artifact/read` request | `src/local_runtime/session/uploads/tests.rs`: commit/copy/publication/deletion gates, restart recovery, historical cwd roots, failed cleanup; `src/model/uploads.rs` exact escaping/order; protocol receipt admission and refusal of manufactured canonical metadata |
| Trace / Trajectory | `chat.spec.ts`, `workflow.spec.ts`: live/historical paging, model/Tool/result, exact native trace identity, running unavailable timing, Workflow/child, reload | Native Trace projection tests and `protocol::trace_reads_are_read_only_and_reconnect_repairs_the_same_native_facts`; `trace-cache.test.ts` 512-entry/4-MiB bound, lifecycle repair, contiguous rebase; `trajectory.test.tsx` bounded virtual DOM, native ordering, selection |
| Todo / Goal / Queue | `composer.spec.ts`: empty→active Todo, fixed seat order, running Goal, exact pending edit/remove, post-claim disabled draft, paused state/revision through reload | `composer-context.test.tsx`: absent/empty/current distinctions, stale Goal CAS without retry; native Goal tests, durable inbound mutation/adoption transactions and `protocol::exact_pending_mutations_are_routed_cas_bound_and_do_not_cancel_attempts` |
| Commands / lineage | `commands.spec.ts`: fuzzy identity, typed model/permission, unknown refusal, Retry branch, original history, exact Fork boundary/upload copies; `uploads.spec.ts` source deletion independence | `commands.test.tsx`: stale cuts, response-loss native reread without replay, pending-inbound admission boundary, obsolete navigation epochs; native fork/cut tests retain immutable historical revisions (old does not necessarily mean invalid) |
| Host / trust | `workspaces.spec.ts`: two Hosts/processes with separate roots, rejection of cross-route/cwd, same native ID isolated between processes, untrusted cwd usable/inert resources, unregister without lifecycle change | `workspace-host.test.ts`, `workspaces.test.tsx`: picker absence and endpoint/epoch admission; native launch trust tests and persistent TrustEpoch publication tests. This is product routing isolation, not an OS sandbox or remote authentication implementation |
| Provider/model | `settings.spec.ts`: User catalog add/edit/delete, native rejection before source write, selection/reset and prospective/runtime distinction; `recovery.spec.ts`: exact CAS conflict and committed response loss; `console.spec.ts`: live frozen Attempt differs from newly desired source | `configuration/settings.rs`: independent source/Catalog/Session CAS, partial authoring/omission, validation, credential withholding, trust locks; `protocol::web08_*`; Settings component tests |
| MCP | `integrations.spec.ts`: User/trusted Workspace stdio ownership, whole-entry shadowing, independent shadowed edit, CAS, deletion and dangling User policy repair after revoke; `recovery.spec.ts`: HTTP/disabled round-trip, no inherited User command, scope/session draft isolation, lost committed save | `configuration/integrations.rs::web09_*`: env/header/reference validation, both scopes, authority ceilings, untrusted exclusion, whole-entry replacement and secrets; protocol and component tests. No browser Probe exists; runtime MCP execution stays native |
| Skills / Agents / Workflows / Extensions | `workflow.spec.ts`: real admitted Workflow Agent child and reload, existing Skill/Agent/Workflow inventory, Workflow selection leaves program and Agent bytes unchanged; `integrations.spec.ts`: activation controls | `web09_resource_definition_and_selection_owners_remain_distinct`, `web09_extension_toggle_preserves_same_scope_siblings_and_replaces_cross_scope`; `session-integrations.test.tsx`; native resource/frozen-child/Workflow tests. Content editing is outside Web v1 |
| Configuration lifecycle | `console.spec.ts` source→prospective→loaded→frozen distinction while provider is gated; settings/integration/trust flows above | Native authority tests reject forbidden scope definitions/security widening before precedence. Source, runtime resources, frozen Attempt/Child/Workflow remain distinct generations; no browser parser/resolver is used |
| Accessibility / responsive | `accessibility.spec.ts`: real keyboard-only connection→Workspace→Session→composer→command→Trajectory→Settings→MCP at 390/820/1280/1600, Escape restoration, focus, reduced motion; existing foundation, composer and inspector tests | Shared horizontal tab keyboard behavior, labelled panels, native modal/menu/popover contracts. Chromium coverage; not a WCAG certification or other-browser result |
| Browser lifetime | `chat.spec.ts`: direct object-URL registry counts through repeated decode/lightbox/reconnect/unmount; `console.spec.ts`: 34 controller releases; `recovery.spec.ts`: scoped drafts across User/Workspace and Workspace A/B | Exact bounded log counters (`diagnostics.test.ts`), cache capacities, URL caps/disposal, stale socket/attachment/page callbacks, listener unsubscribe closures and remount tests. No noisy RSS thresholds |
| Provenance / license | Existing build artifact gate and `check:provenance`; optional `--reference` audit | 74 destination records, 91 original source inputs, 201 inspected paths; pinned SHA, notices for 100 production packages, presentation import boundary. See PROVENANCE.md |

## Authority before precedence

| Domain | Authoring authority | Resolution and lifetime |
| --- | --- | --- |
| Ordinary model selection | User, trusted Workspace, closed Session model selection | Authorized layers resolve in that order; source commits are prospective, loaded/frozen work is separate |
| Provider/model definitions | User catalog only | Bound catalog path is process-owned; Rust parses/validates/serializes; no Workspace/Session definition API |
| Approval / native Tool / MCP Tool policy | User/Host authority | Workspace trust cannot confer security policy authority; MCP definitions and User MCP Tool policy have distinct provenance |
| Credentials | Native User/Host secret authority | Browser sees retained/reference metadata, never secret values. No write-only Host secret endpoint is supplied by this fixture |
| MCP definitions | User plus trusted Workspace | Same-name Workspace replaces the entire User definition; Session has no definition layer |
| Skill/Agent/Workflow resource content | Native resource owners | Source policy/root selections are separate. Frozen admitted resources are not overwritten by source changes |
| Current Todo / Goal / Queue | Conversation/GoalDomain/durable inbound owners | Not configuration. Extension selection is not current domain state |
| Effective | Read-only native projection | Browser shows native winner/lower authored values, generations and frozen facts; it does not merge source documents |

The closed Session vocabulary remains `cwd`, `config`, `model`, `skill_paths`,
`no_automatic_skills`, `no_builtin_tools`, `no_direct_tools`, `tools`,
`exclude_tools`. There is no `rustx.local.toml`, Host metadata override, browser
project configuration store, raw source editor, secret viewer or Harness runtime.
Inspection found only endpoint/wanted-tab hints in localStorage and registration
identity/location/display name/order in Host metadata. No concrete private project
requirement emerged that warrants adding a layer in this issue.

## Synchronization and fault injection

- Provider named gates: `finish-a`, `publish-question`, `settle-chat`, `goal-round`,
  `retry-request-reached`, `workflow-child-admitted`. The real provider validates
  each request before its gate; explicit release orders admission/settlement.
- `wire-probe.ts` forwards actual WebSocket traffic. After a selected real
  `settings/sourcesWrite` result arrives, it drops only that acknowledgement and
  closes transport. Write counts and on-disk/native rereads prove no replay. It
  never supplies a fake snapshot/result. Reconnect is explicit.
- CAS conflicts append a source comment **before** the browser sends its pinned
  revision. Invalid catalog assertions compare exact source bytes before/after.
- Upload/fork/delete checks use native receipts and acknowledgements plus owned
  path bytes. Native tests separately park commit/copy/delete publication with
  channels and injected durability failures; no browser sleep claims a race win.
- Native idle tests use the existing manual clock; trust/catalog/queue tests use
  channels, ordered lock probes and native transactional winner receipts. Client
  tests use held replies, explicit fake socket generations and manual timers.
- Polling waits for authoritative state; timeouts guard liveness only. The E2E
  fixture's only explicit timers guard child readiness/shutdown.

## Fixes required by this gate

1. Browser presentation: tab roles lacked directional keyboard behavior and panel
   relationships. Shared keyboard handling and labelled, focusable panels restore
   semantics without changing native selection/execution ownership.
2. Browser MCP draft encoding: a new draft used an authored empty command even
   when switching to HTTP. `null` now represents absence, matching the existing
   field-clear contract. Rust still refuses mixed/invalid fields before mutation.
3. Dogfood/Host carrier: Node 24 strip-only mode could not load a constructor
   parameter property; explicit property assignment fixes execution. The launcher
   now emits the required Host config and relinquishes its own Host instance before
   the Web carrier starts. No authorization fallback is added.
4. Documentation/provenance: obsolete feature exclusions and the four-icon/98-
   dependency descriptions are corrected in the existing mechanisms.

No Rust semantic defect required a new implementation, protocol DTO or native
owner change in this PR. No compatibility mode, new browser semantic authority,
extra E2E runner or parallel provenance inventory is introduced.
