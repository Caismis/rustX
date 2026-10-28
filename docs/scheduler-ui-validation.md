# Scheduler task UI validation

The task UI follows DeepSeek Harness
`22ddee3792402c3e70ec5f86a63c3e3e895a2f60` while retaining rustX's exact
Session/node binding, Workflow targets, pause/resume, cancellation and recovery.
App Server v62 provides the bounded task catalog, invalidation hints and
metadata-only CAS commands. Scheduler requires Runtime Client v79, SQLite 82,
Session catalog 18, child IPC 37 and Scheduled Workflow compiler contract 3.

## Verification

Validation on 2026-10-27 used Rust 1.98.1, Node 24, the pinned package locks and
the project's Playwright 1.63.0 Noble browser image. No screenshot comparison
tolerance, native execution policy or Office isolation policy was relaxed.

The Rust library and external runs cover the native implementation; the final
Web and browser runs cover the current presentation and control bindings.
The shared descriptor reducer is also covered by the current TUI run in a POSIX
copy of the tracked working files; all client types were checked. Complete verification of the pushed head,
including platform-specific and Office isolation suites, remains the FULL CI
workflow's responsibility.

| Check | Result |
| --- | --- |
| Complete Rust library, all features, including native invocation and Scheduler ownership/status/control regressions | 5,019 passed; 10 ignored |
| External Rust contracts, provider, durable/process/subagent/tool boundaries, configuration and conformance | 683 passed; 10 ignored |
| Cargo/libtest-based test lane coverage on Linux | Passed |
| Recovery proof assertion joins actual native drain completion | 20 consecutive repetitions passed |
| Delegated image/tool/model conformance waits for native idle before manual maintenance | Exact case passed 20 consecutive repetitions; complete conformance 37 passed, three ignored |
| Rust formatting and Clippy, all targets and features, warnings denied | Passed |
| Generated protocol and Web/TUI TypeScript checks | Passed |
| Current TUI regression in a POSIX copy of the tracked working files | 1,308 passed; one conditional integration test skipped |
| Complete Web regression excluding the four Office environment suites below | 3,668 passed; four macOS-only tests skipped |
| Fixed-browser Scheduler, Agent/composer, UI continuity, Trace, native Workflow editor, file-reference and real-server Chat, reading, regeneration and compaction scenarios | 119 passed with strict screenshot comparison where references apply |
| Production Web build, dependency notices, source provenance and i18n ownership | Passed |

The task-specific browser cases cover both locales, both themes, desktop and
narrow layouts, editing, native request payloads, date/clock pickers, exact IANA
zone selection, Workflow records, the header picker and Workbench. The 28 task
references are under
[`web-console/test/e2e/scheduler.spec.ts-snapshots`](../web-console/test/e2e/scheduler.spec.ts-snapshots).
Existing affected references were updated and then compared strictly.
The final browser selection includes 14 Scheduler, 23 Agent/composer, 21
reading-continuity, 46 Trace, five native Workflow editor, three file-reference
and one real-server Chat scenario, plus two each for real-server reading,
response regeneration and compaction. An additional Office-dependent preview
scenario is unavailable locally.
The Workflow narrow
reference includes the Scheduler entry in the collapsed sidebar; no noise regions
or comparison tolerances were added.
Two English desktop Workflow-run references also retain the correct list-row
spacing after renaming reorders tasks of different heights. The approved pixel
changes are confined to the affected list row; the detail panel is identical.

Cold and already-attached navigation both open the original native node.
Changing nodes uses the existing native transition and a fresh editor identity;
reselecting the same pending cold node preserves its local draft. Dirty task
edits survive refresh/reconnection, concurrent revisions require explicit review,
uncertain edits reuse their command identity only on explicit retry, and admitted
edits and deletion survive view disposal. Task editor intent is scoped to the
App Server authority and exact native task, outside the generation-keyed
Workbench. A replacement authority cannot inherit it. Workflow input limits use
compact serialized JSON bytes, including the exact 16 KiB UTF-8 boundary, so
pretty-printing does not prevent a name-only edit. Unmounting the product releases all presentation
subscriptions. Task runs distinguish admission from execution and explicitly
limit history to the latest retained trigger.

An independent recovery action retains the original uncertain command after
Cancel and Workbench remounts. Bilingual browser cases exercise two reconnects,
explicit recovery with the same command ID/payload and a different subsequent
edit; an unchanged read never clears uncertainty. Workflow name/timing edits
retain the complete V1 descriptor after actual V2 configuration adoption and
against cold targets, without composing execution.

Todo and Goal hydrate independently under their native observation owner. An
unchanged Todo revision retains its complete list across Goal changes and inline
capture-budget changes. A changed Todo revision retains the complete list as
refreshing until the replacement is ready, while an unsaved Goal objective draft
remains intact. Process groups use message/block identities through live,
canonical-only and refreshed-transcript publication. Only positively observed
native Attempt membership joins a display head to a group; unknown or independent
heads close the preceding group. The same Tool DOM, keyboard focus and local
disclosure survive all three representations without creating a transcript row
or historical authority. Descriptor-only commitment keeps the previously observed
display blocks and Tool metadata in that same member while the advertised native
block collection is read. The native descriptor head bounds retention; transcript
adoption, eviction and real presentation-scope changes retire it. The shared
descriptor reducer clears only the matching live Assistant MessageId; unrelated,
missing and non-Assistant identities cannot retire it. All four identity controls
cover omitted, null and matching optional AttemptId fields. Tests use 20 individually
bounded Tool calls whose combined display exceeds the native 16 KiB header bound,
hold the block read and transcript independently, and verify one focused Tool
control throughout hydration. No canonical body or transcript row is fabricated.
Visible compaction markers are boundaries for both
durable and display-head membership, including a derived summary after Tool
results in the same Attempt.
Conversation-owned Tool disclosure survives revoked and renewed
attachment admission; a different conversation or server authority is isolated.
Artifact load intent survives same-conversation reconnects while byte leases are
revoked and reread under the new authority. These properties have component and
real Chromium regressions.

Already-released UTF-8 text stays visible while the same logical live source
advances monotonically to a larger exact reference. Replacement sources, versions
and backward frontiers reject the old bytes. Revoked read instances and late
responses cannot publish new text. An explicit logical presentation scope can
retain bytes already released to the reader while fresh admission is pending.
An extension-read failure reports the error beside the retained released prefix.
Trace virtualization initializes from the current viewport offset, preserving
off-tail reading across threshold appends, fold/filter transitions and normal
tail following. Component and Chromium regressions cover these reading boundaries.

Virtual task rows use the same native identity for React and measured heights;
different-height mounted rows reorder without new resize evidence. Count-only
Workflow hydration preserves manual disclosure while genuine running/completed
and focused-body transitions retain their status policy. No-op file-watch
validation keeps highlighted code and keyboard focus without rereading pages.
Natural code-fence completion also retains the same `pre`/`code` nodes while
Shiki replaces streaming tokens with its settled rendering.

First-input acknowledgement retains a single provisional message and attachment
metadata until the client observes its exact accepted MessageId. Unrelated
observed messages do not finish that handoff. File references are released at
acknowledgement; the existing accepted-submission pipeline owns the pending text.
Detached and following readers retain their intent through transient disconnect
and same-conversation reattachment. A same-conversation loading preview cannot
replace already displayed rows. With no usable resume seed, a retained frame
keeps the reading surface mounted while the checkpoint is held, through another
transport loss and while a subsequent retry installs a passive preview. The
still-wanted logical Conversation owns this read-only presentation even without
a target. Old read admission and controls remain withdrawn; a delayed response
from the abandoned socket cannot install its checkpoint. Presentation bindings travel
independently of read sources, so hydrated deferred User and Assistant text
stays mounted even when its native inline header is empty. The retained text
record contains an opaque scope token, references and strings; it carries no
read function or target. Its scope includes client, endpoint/authority namespace,
Session, node, conversation and replay. Fresh admission starts a fresh read,
while real scope or reference replacement clears the released text. Both the
full client/ConversationLive chain and Chromium verify 35,000-character bodies,
unchanged DOM, no reads while revoked, fresh attachment reads and scope clearing.
Passive preview/checkpoint loading
does not detach a following reader when initial rows arrive. The 50-message
browser fixture checks retained DOM and exact message anchors, tail following
and real conversation changes; component tests also isolate a replacement
server authority. Logical conversation and replay changes use
the viewport's existing reset path, independently of revoked read/control
admission.

An admitted save retains its original command ID and draft until its uncertain
outcome is resolved. Explicit recovery acknowledges that exact command to the
editor, advances the baseline and normalizes padded titles and compact Workflow
JSON. Newer local edits and genuinely newer incoming revisions remain intact.
App/Workbench tests cover recovery without Cancel as well as the independent
Cancel/remount path.

## Environment limits

The unfiltered Web run exposed missing Office isolation dependencies in the
development container: `/usr/bin/bwrap`, `/usr/bin/libreoffice`,
`/usr/bin/systemd-run` and a delegated user systemd/cgroup environment. These four
unchanged suites were excluded from the subsequent Web regression, not altered:

- `test/document-converter.test.ts`
- `test/document-host-settlement.test.ts`
- `test/document-integration.test.ts`
- `test/office-resource-boundary.test.ts`

The additional `test/e2e/preview-workspace.spec.ts` browser scenario reached its
Office preview step and reported `Office sandbox unavailable`; it is not counted
among the local browser selection. FULL CI retains this scenario and the
four unit suites under its configured document runtime.

Real Native Web/TUI observation recovery passed using a container-local Provider
environment; the Windows-bound `.venv` directory could not be written by the
container user. Local Rust execution covers the full all-feature library and
every external integration target, with strict all-target Clippy and Cargo-based
lane discovery. The all-target execution gate and platform matrix remain CI
requirements. Final validation used the healthy Docker engine and cached Linux
target volume.
