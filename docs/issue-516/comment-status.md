# PR 517 comment implementation status

Checked on 2026-11-04, Asia/Shanghai, against all nine top-level/review timeline
entries, their IDs and update times, and the complete review-thread response.
No comment was new or edited since the preceding check; the inline-thread list
was empty. This is an implementation inventory, not a public reply or an
acceptance of the remaining latency budget. The current implementation and
verification are recorded in [the interaction report](interaction-roadmap.md).

## Requested PR work still open

| Request | Current state | Source |
| --- | --- | --- |
| Move Ask a side question into the existing right sidebar | Still launched from ConversationHeader and rendered by BtwPanel as a Modal. Sidebar entry/content, draft/result lifetime and coexistence with previews need implementation. | [Comment 6600307712](https://github.com/Caismis/rustX/pull/517#issuecomment-6600307712), [header](../../web-console/src/app/agent/ConversationHeader.tsx), [panel](../../web-console/src/app/agent/BtwPanel.tsx) |
| Widen the file-change hover preview and improve placement/header/scrolling | HoverCard remains 244 px wide, placed to the anchor's right with vertical clamping. A file-change-specific viewport-bounded layout, side selection, readable path/Copy header and long-line handling remain open. | [Comment 6600307712](https://github.com/Caismis/rustX/pull/517#issuecomment-6600307712), [placement](../../web-console/src/presentation/primitives/HoverCard.tsx), [styles](../../web-console/src/presentation/primitives/HoverCard.module.css) |
| Reach 50-ms interaction P95 / 200-ms first complete display across measured workloads/entries | Still unachieved. The prior report retains its actual GPU/proxy/whole-action definitions and the failing budget gate. New outline diagnostics do not substitute for those targets. | [Performance evidence](performance-optimization.md) |

## Roadmap and evidence follow-ups

| Item | Status |
| --- | --- |
| Orthographic/perspective switch | Not implemented; requires projection/scale/picking and compatible camera-state restoration. The roadmap allows it as a separate increment. |
| Observable loading-stage labels | Except for the genuine outline-preparation status, model loading remains generic. Runtime initialization, synchronous CAD import and display preparation are measured internally, but stage transitions are not exposed as distinct product labels. Close/hide cancellation and late-result fencing already exist. |
| Full per-Worker memory attribution | Published cumulative browser-container peaks and bounded resource counters do not attribute JS heap/ArrayBuffer peaks, committed WASM memory and GPU/driver storage to individual phases. Serial-versus-concurrent CAD peak comparison is not published. |
| Google Chrome Stable and optional Beta/Canary lanes | Not run. The separate installed Edge 156 functional/resource lane is explicitly identified; it does not claim a Google Chrome run or certify unknown future releases. |
| Sectioning and mesh measurements | Not implemented. Explicit P1 follow-up scope; clipping/picking agreement, coordinate space and approximation contracts are required. |
| Search/filtering, explosion, synchronized views and version comparison | Not implemented. Explicit P2 follow-up scope. |
| Arbitrary transparency/background controls and ViewCube | Not implemented. Conditional proposals requiring an inspection/usability need and resource/quality evidence. |

These follow-ups are not represented as disabled placeholder controls.

## Implemented feedback

| Comment | Implementation status |
| --- | --- |
| [6582456684](https://github.com/Caismis/rustX/pull/517#issuecomment-6582456684) | Interrupted reauthorization retains the fixed copy; STL classification matches parser input; GLB bounds/spheres use positions; failed image URLs settle; Top/Bottom orbit basis is synchronized; elapsed evidence uses monotonic nonnegative measurements. Existing regressions cover all six findings. |
| [6593676581](https://github.com/Caismis/rustX/pull/517#issuecomment-6593676581) | Redundant adapter copies/scans removed; camera gestures avoid unrelated workspace/React updates; hidden picking and duplicate resize work removed; cancelled native reads settle without unused final payload copies. Client ownership, public APIs and source authority remain. The outstanding measurement/browser work is identified above. |
| [6593794843](https://github.com/Caismis/rustX/pull/517#issuecomment-6593794843) | Current PR increment supplies compact controls, isolate/restore/show-all/focus, distinct Fit/Reset and occurrence properties. Copy-bound inert state and explicit geometry/unit semantics remain. Later/conditional items are listed above. |
| [6595300365](https://github.com/Caismis/rustX/pull/517#issuecomment-6595300365) | Selected GLB scenes have independent nonempty bounds admission. Part ancestry follows whole occurrence-ID segments, including unnamed branches and numeric siblings. |
| [6598650036](https://github.com/Caismis/rustX/pull/517#issuecomment-6598650036) | Final camera metadata cannot reenter body admission during lease retirement; the real full-capacity coordinator regression covers replacement and final-pose preservation. |
| [6599073494](https://github.com/Caismis/rustX/pull/517#issuecomment-6599073494) | Bounded mesh silhouette/boundary/crease lines, individual view icons and direct signed-axis navigation are implemented. Axis lines/labels are the pointer targets; keyboard semantics share that indicator. Outlines default on and retain an explicit off preference. |
| [6600405482](https://github.com/Caismis/rustX/pull/517#issuecomment-6600405482) | Historical checkpoint. Its lossy simplification and shading/MSAA tradeoffs were removed; startup/compiler experiments and separate timing lanes are documented. Its paused state does not describe this active implementation. The latency gap remains open. |
| [6600502539](https://github.com/Caismis/rustX/pull/517#issuecomment-6600502539) | Optional preparation cannot consume the import deadline reserve; lossy simplification is removed. Dense near-limit closed STL and tiny disconnected-solid regressions retain admitted geometry/bounds. |

PR #517 remains draft. Source/API authority changes, merging, auto-merge and
ready-for-review transitions are outside this delivery. The pushed revision
requires its own CI; prior-head success is not certification of a new revision.
