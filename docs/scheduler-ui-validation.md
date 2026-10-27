# Scheduler task UI validation

The task UI follows DeepSeek Harness
`22ddee3792402c3e70ec5f86a63c3e3e895a2f60` while retaining rustX's exact
Session/node binding, Workflow targets, pause/resume, cancellation and recovery.
App Server v61 adds the bounded task catalog and its invalidation hint. Runtime
Client v79, SQLite 82, Session catalog 18 and child IPC 37 are unchanged.

## Verification

Validation on 2026-10-27 used Rust 1.98.1, Node 24, the pinned package locks and
the project's Playwright 1.63.0 Noble browser image. No screenshot comparison
tolerance, native execution policy or Office isolation policy was relaxed.

The native, TUI and full Web results establish the implementation baseline.
The task, reading-continuity and browser rows cover the final UI changes;
complete verification of the pushed head remains the CI workflow's responsibility.

| Check | Result |
| --- | --- |
| Native schedule tests, including catalog paging, search and unattached invalidation | 109 passed; one existing manual performance test ignored |
| Native documentation and runtime-example contracts | 31 passed |
| Rust formatting and Clippy, all targets and features, warnings denied | Passed |
| Generated protocol and Web/TUI TypeScript checks | Passed |
| TUI regression suite in a POSIX checkout | 1,286 passed; eight conditional integration tests skipped |
| Web regression excluding the four Office environment suites below | 3,597 passed; four macOS-only tests skipped |
| Final Web task, navigation, Todo/Goal, process, content and Trace component/integration regressions | 376 passed |
| Fixed-browser Scheduler, Agent/composer, UI continuity, Trace and native Workflow editor scenarios | 91 passed with strict screenshot comparison where references apply |
| Production Web build, dependency notices, source provenance and i18n ownership | Passed |

The task-specific browser cases cover both locales, both themes, desktop and
narrow layouts, editing, native request payloads, date/clock pickers, exact IANA
zone selection, Workflow records, the header picker and Workbench. The 28 task
references are under
[`web-console/test/e2e/scheduler.spec.ts-snapshots`](../web-console/test/e2e/scheduler.spec.ts-snapshots).
Existing affected references were updated and then compared strictly.
The final browser selection includes 11 Scheduler, 23 Agent/composer, six
reading-continuity, 46 Trace and five native Workflow editor cases. The Workflow narrow
reference includes the Scheduler entry in the collapsed sidebar; no noise regions
or comparison tolerances were added.

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

Todo and Goal hydrate independently under their native observation owner. An
unchanged Todo revision retains its complete list across Goal changes and inline
capture-budget changes. A changed Todo revision retains the complete list as
refreshing until the replacement is ready, while an unsaved Goal objective draft
remains intact. Process groups use message/block
identities through live-to-durable publication, preserve expanded DOM and reading
scroll, and carry disclosure into groups that merge across messages. These
properties have both component and real Chromium regressions.

Already-released UTF-8 text stays visible while the same logical live source
advances monotonically to a larger exact reference. Replacement sources, versions,
backward frontiers, revoked ownership and late responses cannot reuse that text.
An extension-read failure reports the error beside the retained released prefix.
Trace virtualization initializes from the current viewport offset, preserving
off-tail reading across threshold appends, fold/filter transitions and normal
tail following. Component and Chromium regressions cover these reading boundaries.

## Environment limits

The unfiltered Web run exposed missing Office isolation dependencies in the
development container: `/usr/bin/bwrap`, `/usr/bin/libreoffice`,
`/usr/bin/systemd-run` and a delegated user systemd/cgroup environment. These four
unchanged suites were excluded from the subsequent Web regression, not altered:

- `test/document-converter.test.ts`
- `test/document-host-settlement.test.ts`
- `test/document-integration.test.ts`
- `test/office-resource-boundary.test.ts`

Real Native Web/TUI observation recovery passed using a container-local Provider
environment; the Windows-bound `.venv` directory could not be written by the
container user. The complete Rust all-target test suite was not run; the schedule
and contract suites above were run against the actual native implementation.

The host D: drive filled during WSL compilation. Removing two obsolete,
rebuildable repository build-cache files recovered approximately 4.4 GB. The
Ubuntu WSL distribution still failed to start after its targeted restart, so
remaining validation used the healthy Docker engine. No runtime data, saved
validation evidence, Docker data or other WSL distribution was removed or reset.
