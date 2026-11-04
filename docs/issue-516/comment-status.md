# PR 517 comment implementation status

Checked on 2026-11-04, Asia/Shanghai, against all eleven top-level/review timeline
entries, their IDs and update times, and the complete review-thread response.
Comment 6623187578 adds a browser-request classification regression;
the inline-thread list was empty. This is an implementation inventory, not a public reply or an
acceptance of the remaining latency budget. The current implementation and
verification are recorded in [the interaction report](interaction-roadmap.md).

## Requested PR work still open

| Request | Current state | Source |
| --- | --- | --- |
| Reach 50-ms interaction P95 / 200-ms first complete display across measured workloads/entries | Still unachieved. The prior report retains its actual GPU/proxy/whole-action definitions and the failing budget gate. New outline diagnostics do not substitute for those targets. | [Performance evidence](performance-optimization.md) |

## Roadmap and evidence follow-ups

| Item | Status |
| --- | --- |
| Orthographic/perspective switch | Implemented on the existing renderer/context, preserving target-plane scale, direction and target. Effective orthographic zoom, picking, resize, standard views and same-copy restoration have regression coverage. |
| Observable loading-stage labels | Implemented from actual reading, CAD initialization, import and display-preparation events. Worker observations are a finite ordered enum sequence, independent of terminal result validation and the original deadline. No guessed parser/tessellation milestones, percentages or ETA. |
| Full per-Worker memory attribution | [Phase samples](review-performance-samples.json) now separate JS heap, backing storage and exported WASM memory, including two genuinely overlapping CAD Workers. These are sampled checkpoints, not peak allocation or GPU/driver attribution; the full peak comparison remains open. |
| Google Chrome Stable and optional Beta/Canary lanes | Not run. The separate installed Edge 156 functional/resource lane is explicitly identified; it does not claim a Google Chrome run or certify unknown future releases. |
| Sectioning and mesh measurements | Not implemented. Explicit P1 follow-up scope; clipping/picking agreement, coordinate space and approximation contracts are required. |
| Search/filtering | Implemented as a bounded local name filter over admitted metadata, retaining matching ancestors without changing scene state or manual expansion. Query resets on panel close or copy replacement. |
| Explosion, synchronized views and version comparison | Not implemented. Explicit P2 follow-up scope. |
| Arbitrary transparency/background controls and ViewCube | Not implemented. Conditional proposals requiring an inspection/usability need and resource/quality evidence. |

These follow-ups are not represented as disabled placeholder controls.

## Implemented feedback

| Comment | Implementation status |
| --- | --- |
| [6623187578](https://github.com/Caismis/rustX/pull/517#issuecomment-6623187578) | Shared request classifiers distinguish actual WASM bytes and executable CAD glue/Workers from Vite URL-import metadata. Cancellation, two-owner reuse and STL lazy-loading assertions retain their original resource and lifetime requirements. Default Vite and production-fixture validation is recorded in [the takeover report](takeover-validation.md). |
| [6620597789](https://github.com/Caismis/rustX/pull/517#issuecomment-6620597789) | Outline depth tolerance is derived from perspective projection and one part per million of eye distance, replacing the fixed NDC offset. Numerical thin-overlap coverage spans three scales. Real OrbitControls input coverage verifies tiny pans render and persist, including retirement before queued input observation through the retirement publisher. Browser pixel coverage is included; current-run results are tracked separately from prior-head evidence. |
| [6600307712](https://github.com/Caismis/rustX/pull/517#issuecomment-6600307712) | Side questions use an inline Workbench tab and existing auxiliary owner, with bounded authority/Session/Conversation drafts, collapsed-sidebar entry and no submission/cancellation on navigation. File-change hover cards opt into a wider viewport-bounded Floating UI surface, full-path/Copy header and scrollable normal-size code. Ordinary hover-card widths remain unchanged. New lifecycle/layout tests and browser cases cover these changes; browser acceptance must use the current source revision. |
| [6582456684](https://github.com/Caismis/rustX/pull/517#issuecomment-6582456684) | Interrupted reauthorization retains the fixed copy; STL classification matches parser input; GLB bounds/spheres use positions; failed image URLs settle; Top/Bottom orbit basis is synchronized; elapsed evidence uses monotonic nonnegative measurements. Existing regressions cover all six findings. |
| [6593676581](https://github.com/Caismis/rustX/pull/517#issuecomment-6593676581) | Redundant adapter copies/scans removed; camera gestures avoid unrelated workspace/React updates; hidden picking and duplicate resize work removed; cancelled native reads settle without unused final payload copies. Client ownership, public APIs and source authority remain. The outstanding measurement/browser work is identified above. |
| [6593794843](https://github.com/Caismis/rustX/pull/517#issuecomment-6593794843) | Current PR increment supplies compact controls, isolate/restore/show-all/focus, distinct Fit/Reset and occurrence properties. Copy-bound inert state and explicit geometry/unit semantics remain. Later/conditional items are listed above. |
| [6595300365](https://github.com/Caismis/rustX/pull/517#issuecomment-6595300365) | Selected GLB scenes have independent nonempty bounds admission. Part ancestry follows whole occurrence-ID segments, including unnamed branches and numeric siblings. |
| [6598650036](https://github.com/Caismis/rustX/pull/517#issuecomment-6598650036) | Final camera metadata cannot reenter body admission during lease retirement; the real full-capacity coordinator regression covers replacement and final-pose preservation. |
| [6599073494](https://github.com/Caismis/rustX/pull/517#issuecomment-6599073494) | Bounded mesh silhouette/boundary/crease lines, individual view icons and direct signed-axis navigation are implemented. Axis lines/labels are the pointer targets; keyboard semantics share that indicator. Outlines default on and retain an explicit off preference. |
| [6600405482](https://github.com/Caismis/rustX/pull/517#issuecomment-6600405482) | Historical checkpoint. Its lossy simplification and shading/MSAA tradeoffs were removed; startup/compiler experiments and separate timing lanes are documented. Its paused state does not describe this active implementation. The latency gap remains open. |
| [6600502539](https://github.com/Caismis/rustX/pull/517#issuecomment-6600502539) | Optional preparation cannot consume the import deadline reserve; lossy simplification is removed. Dense near-limit closed STL and tiny disconnected-solid regressions retain admitted geometry/bounds. |

[The review follow-up](review-follow-up.md) contains current validation, chat
captures and final measured performance gaps. PR #517 remains draft.
Source/API authority changes, merging, auto-merge and
ready-for-review transitions are outside this delivery. The pushed revision
requires its own CI; prior-head success is not certification of a new revision.
