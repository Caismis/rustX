# Sidebar residency, navigation recovery and geometry controls

Validated on 2026-11-06, Asia/Shanghai, after `74e0a3f27`. The earlier review/CI
repairs and their performance limits remain in [the repair report](review-ci-repairs.md).

## User-visible behavior

- The View button and Isometric share the original cube. Reset uses its own
  reset symbol and sits beside Isometric above the six standard directions.
- All six direction icons have actual cube faces and all twelve edges, including
  hidden edges. Highlighted faces agree with the real isometric camera:
  front +Z, back -Z, left -X, right +X, top +Y and bottom -Y.
- The outline tooltip and Chinese label are exactly “轮廓线”. Its icon uses
  four explicit right-angle corners and symmetric central edge dashes.
- One Pan icon toggles left-button rotation/pan. Rotation is the default.
  Pan, Outlines and Isolate use the same selected style. Isolate is one toggle;
  there is no separate Exit isolation button.
- Mouse activation of a coordinate arrow has no rectangular focus outline.
  Keyboard focus remains distinguishable.

Pan uses OrbitControls' supported mouse/touch actions. Its mode survives
projection and up-axis reconstruction, hidden-tab restoration and coarse-to-final
adoption. It belongs to the current copy, not a new renderer or native protocol.

## Bounded geometry residency

The current sidebar scope retains at most two file bodies in total. Visible files
take priority; spare slots retain recently used hidden geometry. A displaced
hidden owner retires before replacement admission. Workbench uses the existing
Dock `keepMounted` hook; the general Dock default and ordinary unselected
HTML/PDF/Office/text lifetimes are unchanged. Managed MIME overrides use the
same viewer-selection function as the renderer, so a misleading geometry suffix
cannot make a Markdown or image body resident.

Parked geometry keeps its exact copy, scene, canvas, camera, Parts state, outlines
and prepared surface. It does not deactivate its already authorized content
owner or repeat native reads/imports, including after large source-buffer transfer.
Hidden rendering, input, animation/fence polling, refinement and new outline work
pause. Resume synchronizes size, DPR and theme. In-flight work retains its original
deadline and cannot restore focus into a hidden view.

Closing, actual sidebar collapse without a visible float, mode/Session changes,
scope revocation and LRU eviction still release resources. Refresh creates a new
copy; authority checks and source-version semantics are unchanged. The existing
selected non-geometry collapsed-body behavior, including HTML iframe state, is
preserved. No persistent parsed-model cache or new resource allowance is added.

## Unrequested Session changes and apparent reloads

A controlled reproduction found that the old hint restored `openViews[0]`, even
after the user selected a later Session. v3 saves the safe selected route
separately from membership. Startup cannot overwrite a valid saved Session while
its attach is pending, and a newer explicit user intent wins over late completion.
Workflows, Schedules and New Conversation keep their own safe route hints.
Draft contents, commands, credentials and observations are not persisted.
Only an authoritative Native `unknown_session` may discard a missing saved ID;
an incomplete catalog or transient connection failure cannot.

Controlled configuration-notification and same-authority reconnect cases did not
switch the selected Session. The user's desktop shortcut was inspected read-only:
`start-rustx-webui.sh` executes the normal `pnpm web` development launcher, whose
carrier uses Vite `createServer`. Frontend edits can therefore trigger hot updates
or React/page remounts. This is a confirmed launch-mode property, not proof that
every reported incident had that trigger. No periodic geometry-reimport mechanism
was found. The desktop script and the user's running processes were not changed.

## Verification

- Full Web/Host suite: **4,442 passed**, four platform skips; 308 files passed.
- TypeScript, production application and geometry-fixture builds, 336 source
  records, 355 notices, runtime/source audits, localization and whitespace pass.
- **55 browser cases pass** through fixed production bundles in installed Chrome
  157 / SwiftShader; two approximately-200-MB STEP opt-in cases skip without their
  external source file. This is separate from the pinned Chromium 153 CI lane.
- Actual native delivery and Workbench file switching preserve canvas identity,
  source-read/import/Worker/context creation counts, Parts expansion and selection.
  Hidden views submit zero draws; collapse and close release all tracked resources.
- Actual page reload and safe reconnect to the same Native authority restore B
  after A → B, without creating a Session, submitting input or persisting tokens.
- Real browser input verifies Pan/Orbit behavior, equal selected computed styles,
  single-button isolation, axis mouse focus and unchanged geometry ownership.
- Real OrbitControls tests cover both projections, up-axis changes, copy defaults
  and progressive handoff. Controlled lifecycle tests cover large transferred
  copies, LRU/two-pane budgets, hidden completion, scope retirement and 120 seconds
  of hidden time without another import.

The browser specs now distinguish active visible bodies from deliberately parked
ones. Total resident/context limits, complete geometry, source-read counts,
MSAA, deadlines and close/collapse zero-resource assertions remain enforced.
No screenshot baseline was updated. The 200-ms first-model / below-50-ms maximum
interaction acceptance remains open; functional completion is not that certificate.
The final pushed revision requires its own CI.
