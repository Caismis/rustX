# Browser CI lifecycle and recovery repairs

Investigated [CI run 42156223858](https://github.com/Caismis/rustX/actions/runs/42156223858)
for `fe8a655a5cc16f1793be4f4f36cc2bf572a929e3` on 2026-11-06,
Asia/Shanghai. Rust, macOS boundaries, TUI, protocol, formatting, browser-input
build and Web/Host/launcher jobs passed. Browser shard one failed; shard two
hit its 45-minute job deadline. Coverage and Merge verification therefore failed.

These repairs change browser tests and their fixture teardown only. No CAD,
Workflow field-saving, configuration recovery or native production code changes.
The Workflow review-boundary tests and the original large-document test match
`origin/main`; Settings production and the recovery test also had no PR changes.
Running the repository's full CI exposed these inherited test orchestration
problems; the evidence does not establish a CAD-to-Workflow functional dependency.

## Host lifetime

Six captured Workflow failures have terminal `fetch failed` / `ECONNREFUSED`
errors from the Node-side Product Host proxy. In the catalog and End-field cases,
the first nested `expect.poll` attempt fails, then a subsequent attempt passes;
the tests continue through their saved-state assertions and screenshots. Those
intermediate trace records are not failed field saves.

The common fixture previously stopped the Host while the browser could still
produce requests. `stopDogfoodPage` now removes intercepted routes and waits for
their active callbacks while the page and Host remain live, then closes the page,
then stops the fixture. Nested `finally` blocks preserve cleanup if an earlier
step fails. It uses Playwright's `unrouteAll({ behavior: 'wait' })`, without
ignoring request errors, adding retries or changing production HTTP handling.
Unrouting must precede page closure: on the pinned Playwright API, unrouting after
closure can drain callbacks but reject when updating interception patterns.

All 51 native Workflow teardown sites across 18 spec files use this owner order.
Intentional-disconnection scenarios outside these Workflow cases are unchanged.
The new real-Host browser regression holds a proxy request, begins teardown,
proves later requests cannot enter the removed Node proxy and verifies the held
response arrives before page and Host retirement. Its assertion wrapper still
stops the fixture if an assertion fails. A separate controlled old-order probe
reproduced the exact refused-connection error before the repair.

## Configuration recovery readiness

Both configuration writes in the captured CFG3 failure committed once in Native.
The failure was a test helper's immediate visibility check after reconnect:
the authoritative source response had not yet restored the retained MCP editor,
so the helper skipped Back and later waited for a list item behind that editor.
The trace's final source response follows that visibility check.

The helper now distinguishes opening the MCP list from returning from a known
retained editor. Returning awaits Back and the list heading. The actual writes,
three intentionally lost responses, single reconciliation and no-replay checks
remain unchanged. A held real native source-read response reproduces the failure
with the old helper and passes with the corrected helper. No timer, fabricated
configuration projection or production recovery change is introduced.

## Large resource tests

The original test placed Workspace and Global authoring in one 120-second budget.
The CI trace shows a successful Workspace save taking approximately 97 seconds
from Save click through the final clean-state assertion. Original code/schema
checks pass; the test then reaches Global loading with too little time remaining.
The initial loading snapshot's missing-file placeholder does not prove a terminal
file-not-found response. Both scopes complete in the unchanged local test.

Each scope now has its own fixture/test with the existing 120-second limit.
Both large files remain present in each inventory. Source documents remain above
3 MiB, check schemas above 1 MiB, control frames at most 1 MiB; code, schema,
native-save, provider-count and error assertions remain. An added saved-title
assertion verifies that the actual edit persisted. No performance threshold or
timeout is raised. Independent scope checks locally passed in 39.4 and 36.5 seconds.

## Keyboard activation readiness

The broader local regression group exposed another test-side readiness race in
the branch template case. On reopening, React Flow still had the node temporarily
`visibility: hidden` for measurement. `locator.focus()` followed by `press('Enter')`
does not wait for visible actionability; no Inspector opened. The saved node still
contained the correct `args.count` token. The shared local activation helper now
awaits node visibility before exercising the same keyboard interaction. It does
not replace keyboard activation with a click or change template persistence.
This case passed in the original CI and is recorded as a local hardening finding,
separately from that run's terminal failures.
The Human Input case had the same hidden-node condition immediately after a
narrow-viewport resize; it now waits for visibility before focus/Enter as well.

The keyboard/node-move test similarly read a drag coordinate immediately after
opening the Inspector, before the canvas repositioned the selected node. The
screencast shows the drag selecting Inspector text instead of touching the node.
Trial-click actionability at the intended drag point now precedes reading its
bounding box and sending the same real mouse gesture. It waits for a visible,
stable, unobscured target without replacing the drag or changing its assertions.

## Evidence and validation

The original downloadable evidence is retained by GitHub:

| Artifact | SHA-256 |
| --- | --- |
| [Browser evidence shard one](https://github.com/Caismis/rustX/actions/runs/42156223858/artifacts/13476930652) | `ce23f060e59afacbabefbcdd7d7809850627e986554050c8cb95d6b7f07268f1` |
| [Browser evidence shard two](https://github.com/Caismis/rustX/actions/runs/42156223858/artifacts/13477112032) | `f8c7929ea4562cc6e82d66aa48e4bbe9a284301147ab8cabe5bd948aa6ccc95f` |

Validation results:

- The two initially failed Workflow files plus the Host-lifetime regression:
  12 passed. Repeated original failures, lifecycle and visual checks: 30 passed.
- CFG3 recovery: three ordinary repetitions passed; the real native read held
  across the problematic navigation failed before the fix and passed afterward.
- Both large-document scopes passed independently with all payload/frame checks.
- Final branch spec: seven passed. Final keyboard drag: two repetitions passed
  after the unchanged baseline reproduced a zero-distance drag. Final Human
  Input fields: both widths passed.
- TypeScript, 336 provenance records, localization and whitespace checks passed.
  Production directories have no changes in this increment.

The broader 99-case installed-Chrome diagnostic finished with 94 passed and five
failures. Three failures led to the visibility/drag fixes above and their passing
focused verification. One node-audit menu detached during a click; the unchanged
case passed its independent follow-up, so no production/menu workaround is
claimed. One existing `workflow-editor-linux.png` reference differed in the
non-authoritative local browser; its baseline and comparison bounds remain
unchanged. This initial diagnostic is not represented as an all-green final-head
pinned-browser run. Its cases include the four unchanged child-recovery checks
selected by the filename filter.

Local browser runs use installed Chrome 157 / SwiftShader as a supplementary
diagnostic. Docker/Podman is unavailable on this host, so these runs cannot
certify the repository's immutable pinned-browser screenshot lane. No screenshot
reference is updated. The strict geometry performance gates remain open under
[issue 534](https://github.com/Caismis/rustX/issues/534). Final-head CI must be
reported separately from the prior head's passed native jobs.
