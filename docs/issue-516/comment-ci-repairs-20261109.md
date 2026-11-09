# PR 517 comment and CI repairs

These repairs address [review 6736369065](https://github.com/Caismis/rustX/pull/517#issuecomment-6736369065)
and the failures observed in [CI run 42582809101](https://github.com/Caismis/rustX/actions/runs/42582809101)
at `f03f842f5be626d1394afd1a9ab0bfc19ab37b4a`.

## Behavior

- Delivered geometry receives Home activation through `PreviewWorkspace`, combined
  with pane and tab visibility. Settings, Workflows and Scheduled Tasks park the
  existing viewport. Returning Home retains its canvas, scene and fixed copy.
  The real-App route regressions cover all three destinations; existing geometry
  tests cover surface settlement and deferred outlines while hidden.
- Cold-MCP browser fixtures seed the current v3 saved Session route. Their exact
  MCP-call counts remain authoritative. Held Workspace-read tests require the
  disabled “Preparing to send…” action before the enabled “Send” action, retaining
  draft, selection, focus and composer geometry assertions.
- A committed Session returning Home while disconnected retains a restore intent.
  Reconnect and page reload restore that Session. An uncommitted cold selection
  keeps its existing ephemeral intent; newer navigation still wins. The component
  regression first reproduced the incorrect saved New Conversation hint.
- Narrow conversation headers use an 8px utility gap, preserving readable current
  child titles at 390px. Native child inspection keeps its existing width and
  horizontal-overflow assertions.
- Browser fixture retirement fences new Host work, drains all admitted Node proxy
  callbacks, then removes interception and closes the page and Host. This avoids
  Playwright 1.63 removing interception after the first of several callbacks
  completes. A staggered two-request regression reproduced `Route is already
  handled!` before the repair; both lifecycle regressions pass afterward. Console
  evidence uses Playwright's per-test output directory and the same retirement
  helper.
- The closed-peer Native framing test creates its socket pair after exec in an
  isolated single-test process. Unrelated tests cannot fork a copy of that peer;
  dropping it closes the final descriptor before either original error assertion.
  The child must report exactly one executed test. This uses the repository's
  existing test-process isolation pattern and preserves production framing behavior.

## Screenshot references

The Shell source fixture now publishes its healthy prospective `policy` mode.
Shell and Scheduler references display that Policy value and the captured native
model where available. Seventeen changed PNGs were generated from stable captures
in the immutable Playwright 1.63.0 Noble image
`sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`,
using Chromium 153.0.8010.12 and Liberation Sans for Helvetica.

The [reference audit](comment-ci-reference-diffs.json) records before/after hashes,
changed pixel counts and exact bounds. Changes are confined to the permission and
model controls. Screenshot thresholds, stabilization, retries, test deadlines and
the measured rasterizer-noise policy remain unchanged. The Workflow editor graph
passes its existing reference without a baseline or noise-policy update.

## Verification

- 469 related React/jsdom tests pass across 36 files. These ran in an isolated
  Windows configuration with the production React/alias settings; Host startup
  prerequisites were excluded from that component check.
- All six original framing tests pass against the production framing definitions
  extracted verbatim into a small Linux harness with the repository's pinned nix
  dependency. This checks the actual test file and framing implementation, rather
  than certifying the complete Native or macOS suites.
- Native executables and the document Artifact fixture build successfully. Rust
  formatting, Web TypeScript, production build, 347 source provenance records,
  355 production notices, geometry runtime/source assets and localization pass.
- All 27 distinct pinned-browser cases pass in one final run with update mode
  disabled, the original 120-second test deadline and zero retries. Coverage
  includes real Session detach/reopen, disconnect, page reload and approval
  recovery; six MCP navigation/recovery cases; both held Workspace-read widths;
  Shell and Scheduler references; Workflow editing and child inspection; and
  single/staggered Host retirement.

The original macOS descriptor-watch timeout did not recur in the inspected retry.
At `f5593279d`, the full Web/Host, TUI, Linux, browser and coverage jobs passed;
the macOS suite passed 4,216 tests but retained the closed-peer failure. That
[counterexample](https://github.com/Caismis/rustX/actions/runs/42602221278/job/129109346163)
establishes that shutting down a peer with another live descriptor is insufficient
for this assertion on macOS. Apple's [shutdown contract](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/shutdown.2.html)
only describes operations on the specified endpoint, while the
[Unix-domain send and disconnect implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_usrreq.c)
distinguishes send closure from peer receive state. The isolated process establishes
actual final close. Current-head CI owns the full platform-specific verdict.

PR 517 remains draft. The existing 200ms first-model and below-50ms maximum-input
acceptance gates remain open; these correctness and CI repairs do not certify the
wider geometry performance matrix.
