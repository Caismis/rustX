# Independent implementation review

The user requested an independent sub-agent review before PR creation. The
read-only `review_issue_504` agent reviewed the complete implementation and the
issue requirements, with follow-up passes after repairs. It did not author or
modify implementation code, run heavy builds, commit, push, or create the PR.

## Findings resolved before the final review

| Finding | Resolution and regression |
| --- | --- |
| Submitted create could overwrite a concurrent visible Session attachment. | Register the unique loading flight and convert its creation reservation under the catalog publication lock. The existing multi-client visibility regression now exercises submitted and inert creation, including durability uncertainty, and checks one composition/incarnation. |
| A committed creation carrying a durability diagnostic could discard its original owner/clock. | Retain the committed Session's unique initialization owner; the Web continuation still stops before attach/input on that diagnostic. A real MCP regression checks one handshake and the same deadline. |
| Initialization failure before attach could lose the Send clock. | Retain the terminal result whenever the shared initialization has accepted submission intent. Automatic attach/reconnect returns the same result; a retry's own failure is retained too. Native regression exercises repeated automatic attach and two failing explicit retries. |
| Physical Workspace identity could become stale during capacity/readiness waiting. | Compare device/inode and captured bindings after capacity waiting and recapture current inputs after readiness waiting, before transfer. Ready and queued replacement regressions prove stale physical ownership is rejected. |
| Definite rejection/discard left an editable draft sealed. | Observe the real first-submission owner; release and retire only after authoritative rejection or explicit discard. Unknown outcomes remain sealed across reconnect. |
| Retained failure had no reachable client recovery. | Explicit initialization recovery replaces only the known failed native owner. Existing Web Open Session and TUI reselection invoke it without replaying creation or input; prepared attachment observation failure still reuses that attachment. |
| Delayed Web recovery could overwrite newer navigation. | One captured XState route/control fence gates final recovery dispatch, post-ACK navigation and error publication. Delayed authorization and delayed reply regressions preserve the newer Workspace route. |
| Repeated close after completed drain could restart an empty supervisor. | Make native scope close idempotent and schedule retirement only for an existing current slot. The existing drain regression now closes twice and drops the connection before synchronous completion, without yielding. |
| The post-visibility test hook could starve the submitted loading flight. | Run that blocking, test-only hook on the blocking pool at the same boundary. Production ownership/spawn ordering and the 30-second test bound are unchanged. |

## Pre-PR reviewer conclusion

> No remaining actionable findings or code-level blockers in the reviewed
> implementation. The previously reported ownership, deadline, physical-Workspace,
> recovery, and navigation issues are addressed.
>
> Final review was read-only; no builds or tests were rerun. Final-head FULL
> verification and completion of the issue's evidence report remain required.

The primary agent separately owns executed validation and the evidence report.

The final delta review also confirmed that the committed-creation guard removal
preserves the original preparation/deadline while the Web honours its diagnostic.
It found no new actionable issues. The additional credential/model regression
proves invalidation and unchanged-input reuse without changing implementation.

After #505 landed, the same independent reviewer checked the rebase onto
`1395faf0d00f474f5a79d809b7a91d6fc99c679b`. It found no actionable
integration issues: the ownership/recovery changes and upstream preview behavior
are preserved, all code readers use v58, and the updated provenance matches the
integrated source and preparation dependency. This pass was also read-only;
integrated FULL verification remains the primary agent's responsibility.

Full local verification subsequently exposed the repeated-close defect and the
blocking-hook scheduling issue above. The independent reviewer confirmed both
causes, then reviewed the applied repairs and stronger drain regression. Its
conclusion was: "No remaining actionable findings in these deltas." Focused and
full executed revalidation are reported separately in the evidence report.

## Post-PR review and independent follow-up

External review of PR #506 at
`e6d6bca8eb2c167bcef62dd1485644aee229fd56` identified two defects missed by the
pre-PR review: ordinary creation acknowledgement could cancel a reserved
preparation before validated transfer, and terminal WriterAbsent failure receipts
consumed physical runtime residency. Both required changes before merge.

A fresh read-only sub-agent, `review_followup_504`, reviewed the repair against
that commit and checked surrounding navigation, authority and recovery paths.
It also identified the related presentation/discard/isolation cases below; each
was repaired and re-reviewed. It authored no implementation, edits, builds,
test executions, commits or pushes.

| Finding | Repair and regression |
| --- | --- |
| Creation ACK could cancel before validated transfer. | Match the selected Session to the application-lifetime `FirstSubmissions` binding and retain only its already-sealed selection. Genuine XState/port tests deliver the ACK with attachment held; a native composition barrier independently proves the ACK can precede transfer. |
| Settings and same-Session selection replace disposable navigation interest. | The first-submission owner, rather than `draftSession.intent`, retains exact identity. Both held-attachment orders preserve preparation and one accepted input. |
| A known-Session discard could start unused preparation. | Retire that binding without returning it to unsent-draft preparation. Create/attach-failure/discard regression proves no new Host observation or native preparation. |
| An older rejected/discarded Send could retire the current different draft. | Retire only a matching selected binding. Late A rejection/discard leaves B's exact generation and preparation unchanged. |
| Revisiting an older submitted Session could start fresh unused preparation. | Session navigation follows the original binding without a Workspace; it can retain a sealed selection but cannot create one. A/B submission then cold return-to-A regression proves no new preparation. |
| WriterAbsent failure retained a physical residency slot. | Count only Loading, Loaded, Unloading and pending creation owners. Retry, submission intent and replacement each reserve under the same registry lock before changing a failure to Loading. One-slot real missing-tool regressions cover other-Workspace creation/cold load, unchanged automatic errors, capacity refusal and successful explicit recovery after release. |
| Browser controls could conceal ACK-before-transfer ordering. | All seven process/evidence cases deliver creation replies normally. Manual title suppression holds only the later native attachment reply, after transfer. |
| The creation-visibility regression raced legitimate post-ACK catalog publication in FULL CI. | Use the existing per-Conversation publication gate before releasing creation. Preserve the exact original revision/model/instructions/incarnation assertions while parked, then release and prove one legal revision advance, runtime/retained agreement, unchanged selection and instructions pending explicit adoption. No production change, sleeps or relaxed assertions. |

The follow-up reviewer concluded:

> No remaining actionable findings in the current fixes compared with
> `e6d6bca8eb2c167bcef62dd1485644aee229fd56`.

The reviewer confirmed exact binding isolation, retention without fresh Session
prewarming, and atomic residency admission on all recovery paths. This review
was read-only; executed validation is recorded by the primary agent separately.

After FULL CI exposed the post-ACK test ordering above, the same follow-up
reviewer independently confirmed the legitimate publication path, reviewed the
applied gate and stronger post-settlement assertions, and found no actionable
findings in that test-only repair.

Full local browser validation also exposed an unrelated redundant Office-test wait: the
complete PDF was rendered, but Playwright `response.finished()` remained
unresolved. The follow-up reviewer inspected its trace and the Host terminal
envelope/parser path. It approved removing only that extra wait while preserving
the exact request/status matching and all content, page-count, layout, zoom and
retention assertions. No production defect was confirmed; the underlying browser
transport-observer cause is not established. This was read-only review without
builds or test execution; the primary agent ran all four Office cases and the
complete browser suite afterward.

## Subsequent history and navigation state review

External review at `63c0925716c61ab3019cd6225ee9aa360855be73` confirmed the
earlier preparation-ownership and physical-capacity repairs, then identified two
additional boundaries missed by prior review: retained WriterAbsent failure
blocked committed history, and Settings before creation ACK left admitted input
preventing New Conversation.

A fresh read-only sub-agent, `review_state_boundaries_504`, audited every
`FailedSubmission` consumer, cold/live historical read fencing and first-submission
phase transitions. It found one further related discard path and reviewed all
three applied repairs. It authored no files and ran no builds or tests.

| Finding | Repair and regression |
| --- | --- |
| Retained initialization failure blocked history/detail even after writer absence. | `history_observation_cut` treats only absence and `FailedSubmission` as cold. A real completed request, physical unload and failed strict MCP cold initialization preserve exact committed history and trace detail. Repeated attach/load retains the identical error; discovery/list/tool/provider counts remain unchanged. Existing Loading/live read fencing remains covered. |
| Settings before creation ACK stranded an admitted draft binding. | Permit admitted input to enter the existing new-draft action. Tests use the actor's real numeric binding and separately held create/attach replies, then prove fresh binding, B selection/preparation, one creation and no message replay. The prior navigation fixture now also uses its real binding. |
| Known-Session failure discarded after Settings-before-ACK could reuse its sealed binding. | Draft identity is reusable only without a known Session, including the `DRAFT.WORKSPACE` preservation path. Five actual-owner cases cover admitted or failed/discarded input through New Conversation, Workspace selection and the editable composer picker. Unresolved input remains protected. |

The final review concluded:

> No remaining actionable findings in the current delta against
> `63c0925716c61ab3019cd6225ee9aa360855be73`.

The reviewer confirmed the earlier preparation ownership and residency repairs
remain intact. Executed focused and full validation is recorded separately by the
primary agent in the validation receipt.

The subsequent full browser run exposed a separate document-test observation
failure. The fixture trace contained the complete 9,661-byte PPTX multipart
response, including its 8,991-byte PDF, but CDP response-body retrieval failed
before any navigation or page closure. The specific Chromium cause is unproven.
The same independent reviewer checked the trace and the applied test-only repair:
the existing proxy fetch buffers once, exposes those exact status/headers/bytes
through a synchronous fixture hook, and passes the same result to `route.fulfill`.
The test retains browser response observation, correlates the operation ID and
headers, decodes the captured terminal bytes, and retains every digest, file,
PDF-byte, rendered-preview, original-download and cleanup assertion. Streaming
observation and write gates are unchanged. Its conclusion was: "No actionable
issues or weakened assertions in these two E2E diffs." No additional fetch,
retry, fallback, timeout change or production change was introduced. This review
was read-only; focused and full browser execution are recorded separately.
