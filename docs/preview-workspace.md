# Session preview workspaces

The browser `PreviewWorkspaceOwner` is the sole owner of logical document tabs,
occurrence lifetime, pane membership/selection, active pane, requested fullscreen,
split ratio, saved view metadata and selected resource leases. It retains at most
four Session workspaces in memory. `App` routes explicit Preview and Download
intents to this owner; `RightPanel` supplies the existing Inspector/preview seat.
There is no server tab registry, browser storage persistence or generic docking
runtime. Reload drops the workspace.

## Identity and retirement

A compatible workspace requires exact Session ID, App Server client generation,
server authority revision, Product Host authority revision and all fields of the
attachment target (Session, Conversation, runtime incarnation and attachment ID).
The owner subscribes once to the client and Host authority. Observable replacement,
detachment or deletion synchronously disposes incompatible leases and removes the
incompatible logical workspace before publishing its next snapshot. An ordinary
Session selection keeps other compatible logical workspaces and retires all their
resources. Returning reauthorizes and rereads only selected visible documents.

Inside that exact scope, `samePreviewSource` compares the closed source union:

- Managed Artifact: Artifact ID. The enclosing exact workspace supplies the
  applicable Session/Conversation authority.
- Session file: canonical Tool message ID, delivery index, and every field of the
  original SessionFileReference: original Conversation, device/inode, relative
  path, name, MIME, and optional description (missing and null normalize equally).

Display name, object equality and serialized JSON are never tab identity. A
repeated declaration remains distinct. Equality is stale-source detection, never
read authorization. Each reread/conversion/download uses current native checks;
mutable Session files remain references to current authorized original bytes.

Opening an already open exact source reveals/selects its existing occurrence and
pane; it does not move or duplicate it. Explicit close deletes that occurrence.
Reopening allocates a strictly increasing workspace-owner-lifetime numeric occurrence ID.
The ID is never reused across Sessions, authority changes or a full close.

`FilePreviewCoordinator` coordinates the selected Session's authorized transports;
`FilePreviewLease` owns one occurrence's AbortSignal, read/derive demand and URL.
Leases are separate from logical tab records. Closing, selecting another tab,
collapsing, showing Inspector, hiding a narrow pane, Session selection and scope
retirement synchronously abort the old lease before publication. Async work must
still match its captured authority, target, exact source, occurrence and active
lease. Late success releases its resource and cannot populate a reopened source.
PDF owners additionally listen to lease abortion, immediately retiring workers,
render tasks, canvases and text, without waiting for React unmount.

## Finite budgets

`src/client/preview-policy.ts` holds the workspace policy. The lower resource
owners retain their existing independent byte/security limits.

| Item | Bound / behavior |
| --- | --- |
| Logical tabs per Session | 8; ninth open reports a localized limit and preserves all tabs |
| Retained Session workspaces | 4 nonempty compatible workspaces; fifth reports a limit, without silent eviction. Close the tabs in a retained Session to free its workspace |
| Logical panes / active preview bodies | At most 2 / 2 |
| Hidden tab or background Session | Metadata only; zero active leases, URLs, workers or canvases. Retired transport work holds only bounded settlement admission until cleanup |
| Selected preview original URLs | At most 2, one per visible occurrence |
| Download | At most 1 transient original-byte lease/URL, separate from the two pane leases |
| Aggregate original preview/Download URLs | 3; synchronous retirement/revocation, exactly once |
| Original transfer concurrency | 2 shared permits (an active derivation reserves one); at most 3 pending visible/Download intents, removed on cancellation |
| PDF-backed owners/workers | 2; includes DOCX/PPTX derived PDF, each retaining all prior page/canvas/text/watchdog bounds |
| Host derivation | 1 active; at most 2 visible waiting intents, no retries or background conversion |

The existing transcript's inline managed Artifact image owner is separate from
preview body leases and keeps its existing 2-transfer/16-URL limit. It does not
mount document viewers. Native/Host original reads remain limited to two.
[File delivery](file-delivery.md) and [document previews](document-previews.md)
define exact bytes, pixel/canvas, parser, Host settlement and security limits.

Download is a separate typed intent. It neither creates/selects tabs, changes the
active pane nor opens/collapses/fullscreens the panel. It reacquires original
bytes with current authorization and original filename, clicks one transient
anchor, and revokes its URL in an owned finally path. Derived Office PDF is never
the canonical download. Download remains available with two populated panes.

Office demand is bounded and serial across selected Session coordinator changes.
Waiting canceled demand disappears immediately. Active canceled demand retains
admission until Host physical settlement. The private HTTP carrier acknowledges
exact operation admission before accepting exact cancellation and retains the
original terminal response through cleanup; a fetch abort is not settlement.
The private native file-read socket joins its exact admitted read after cancellation
before acknowledging clean close. The Node carrier waits for that acknowledgement;
closing a socket alone cannot prove that the native read permit was released.
Unknown transport/physical retirement fails admission closed. This narrow native
lifecycle repair changes no public protocol method/schema or file authority. No
generic scheduler or automatic conversion retry is added.

## Pane geometry and presentation

The owner carries a presentation geometry epoch separately from logical panes.
Collapse, Inspector, Session selection, loss of the current workspace and zero
visible measurement invalidate geometry. Hidden callbacks cannot authorize a
measurement; callbacks captured in an older epoch are rejected. On reveal,
unknown geometry admits only the active pane. A fresh positive layout or
ResizeObserver measurement of the visible workspace in the current epoch is
required before `splitFits` can admit the second pane. No hidden stored width
is resource authority. Logical membership, ratio, selections and view metadata
survive invalidation unchanged.

The actual workspace is measured by ResizeObserver. Splitting requires two open
documents and at least 608 px: two 300 px working panes plus an 8 px divider. Each
pane has a horizontally scrolling, min-width-zero tab strip and a fixed move
control. The action remains keyboard reachable with aria-disabled and a localized
reason when there is insufficient room, only one tab, or already two panes.
Split moves the selected document to a second pane and selects the nearest
remaining sibling in the first. Opening targets the active pane. Move explicitly
transfers a selected tab to the other pane. Split/move remount the transferred
body under a fresh lease while preserving its occurrence and view metadata;
that body rereads/reauthorizes on its new placement. No empty/default document is invented.

The saved ratio is finite and bounded by 20–80%, further clamped by the measured
300 px minimum per pane. Pointer capture coalesces visual updates to one frame;
pointer release commits one ratio, while cancellation/lost capture restores the
pre-gesture ratio. The vertical ARIA separator exposes meaningful percent bounds
and current value; Left/Right adjust by five percentage points, Home/End choose
measured bounds. Escape cancels an active divider gesture before panel handling.

Requested fullscreen is logical Session state. Normal/fullscreen changes the
geometry of the same keyed content subtree. It requests a layout commit in a new
epoch without acquiring any resources from the prior width. The synchronous
layout effect measures the new coordinate system before reconciling leases;
existing bodies remain keyed through this commit. The caller uses `flushSync`;
if no positive layout witness is supplied, the owner conservatively reconciles
before the fullscreen transaction returns. If the measured visible set
is unchanged, its leases are retained without rereading. A zero measurement
retires the second lease; a narrow measurement does likewise. This synchronous
visible-to-visible commit is distinct from hiding/revealing, which retires leases
before publication and always starts conservatively. The existing
AppFrame normal right-column track remains reserved while fullscreen is requested.
At narrow geometry only the active pane is visible. A Switch preview pane control
reaches the other document; hidden resources retire. Logical membership and ratio
remain, and widening restores both selected panes. Collapse is explicit and never
reversed merely by widening.

Opening Preview selects the preview mode. Inspector toggles between its existing
content and the retained preview workspace. Inspector keeps its existing behavior
across Session selection. Preview/Inspector toggle is a temporary content-mode
switch and retains the workspace's expanded intent. Explicit Close/Collapse, including Close inspector,
always records `expanded: false` for the current Session and releases resources.
Session switching, authority-stable refresh, Inspector toggle-off and widening
cannot reopen that explicitly closed panel. Explicit Preview/Reopen intent sets
expanded again and restores the retained tabs and view metadata. Only explicit tab
close deletes an occurrence (scope invalidation also retires incompatible state).

## View metadata, keyboard and focus

Tab records contain only inert display/source metadata, pane ID and finite view
fields: text/Markdown/image body scroll, wrap preference, PDF page/zoom/page scroll,
Workbook sheet/row window/scroll, and HTML rendered/source mode/source scroll.
Opaque-origin HTML iframe internals remain inaccessible; HTML source and the
outer document scrollport are retained without weakening the sandbox. Scroll and control interactions continuously commit these fields; preservation
does not depend on unmount cleanup after synchronous lease retirement. View state
is bounded metadata rather than live document runtime. Reactivation can reflect
changed mutable file content, so viewer selections clamp to the current document.

Tab keyboard navigation uses manual activation: Left/Right/Home/End move focus;
Enter/Space select; Delete closes the focused occurrence. Clicking or focusing a
pane makes it active. Closing a selected tab chooses its right sibling first,
otherwise the left. Closing an unselected tab preserves selection. An empty pane
is removed; the final tab closes the workspace with no replacement document.
Close focus goes to the surviving selected tab/pane. Final close or collapse
returns to the still connected, visible opener; otherwise it uses the preview
toggle or the stable Conversation region. Focus restoration uses preventScroll.
Fullscreen/restore retains the focused DOM control. Controls have visible focus;
reduced motion uses the existing RightPanel/AppFrame media policy, and new split
geometry adds no animation.

Preview Escape handling is scoped to its subtree and bubbles after menus. It
respects defaultPrevented, IME composition and modifiers; held-key repeats do not
perform another action. Escape restores explicit fullscreen first, then collapses
on a separate press. It prevents and stops handled gestures. Composer cancellation
remains scoped to its own focused editor/priority sequence; no new document-level
Escape listener is installed.

`ChatViewport` remains the only Conversation scroll writer. Preview changes flow
through existing frame geometry and ResizeObserver measurement; no preview code
writes transcript scrollTop, invokes scrollIntoView or changes follow/history
intent. Controlled geometry and real-browser tests cover following, detached and
historical reading, including user scrolling before a pending correction.

## Removed model and reference

The `artifactPreview` state, `FilePreviewResources` single-selected-view owner,
source-JSON keys, Preview callback/download flag and Blob-link Download path are
removed. Native filesystem, Artifact authority, Agent/upload execution and document
security owners remain in their existing layers.

The pinned Harness source was inspected for Session-local surfaces, occurrence
retirement, same-tree fullscreen, measured bounded split and close focus. No new
Harness source was copied. The exact inspected files and excluded docking/plugin/
registry/default-document machinery are in [issue-441-validation.md](issue-441-validation.md).
Only hashes/dependencies/treatment of already attributed local presentation files
were refreshed in the provenance inventory.
