# File-preview behavior alignment

Reference: DeepSeek Harness `687f282e8625c6b8d11ff58211a90524be2e4317` (dsh 0.2.1-rc.2). rustX inspection baseline: `006d1bb4ae5e6de9adb1ba1ae7af53e625dd9c71`, the development head of upstream PR #505.

This comparison was derived from the pinned implementation and rustX's existing
contracts before changing the preview owners. The final column defines the
implemented contract, subject to the explicit boundaries below. Matching a
loading mode alone does not establish business alignment.

## Primary sources

- [Preview body](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/TextPreview.tsx), [asynchronous face](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/face.ts), [tab store](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/store.ts).
- [Renderer matching](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/document/registry.ts), [unsupported binary suffixes](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/document/unviewable.ts), [failure recourse](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/failure-line.ts).
- [Resource dependencies](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/document/resource-group.ts), [metadata provider](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/client/provider.ts), [shared target feed](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/client/change-feed.ts), [native change producer](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/changes.ts).
- [Office browser cache](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/office/cache.ts), [Office wiring](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/office/index.ts), [configuration defaults](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/config.ts).
- [Delivery cards](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-deliverables/src/client/Deliverables.tsx), [presented-file actions](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-deliverables/src/client/PresentedFileCard.tsx), [HTML body](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/html/HtmlBody.tsx), [spreadsheet registration](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/excel/index.ts).

The exact inspected-source hashes are recorded in
`web-console/source-inventory.json`; links above pin the authoritative commit.

## Behavior matrix

| Behavior | DSH at the pinned commit | rustX before this change | Implemented contract / justified boundary |
| --- | --- | --- | --- |
| Preview entry | Presented files and the file tree open the same Session file-resource preview. | Delivery/Artifact and Workbench files have separate loading owners and different behavior. | Share preview business rules and presentation while preserving each existing native source adapter. No path-based authorization or cross-source cache identity. |
| Identity | Session/resource address identifies content; tab occurrence owns independent view state. | Committed declaration, index, full file reference, Child namespace and exact attachment/Host scope identify delivery previews. | Preserve the stricter native identities. Distinct declarations must not merge merely because paths match. |
| Renderer selection | Suffix matching, fixed builtin priority, explicit viewer choice, text fallback for uncertain suffixes. | Delivery MIME routing differs from the Workbench selector; code and source alternatives are missing. | Share one file-suffix policy and compatible text pages. Preserve native typed managed-image/Markdown semantics because their display names may be opaque IDs or alt text; image bytes still require raster admission. No plugin registry or arbitrary extension execution. |
| Unsupported binary | A known binary suffix without a renderer does not start a text read. | Some unsupported files start an unnecessary original-byte read. | Show a localized unsupported state before content acquisition; retain authorized original Download/Open actions. |
| Initial text | First page starts at line 1; pages are keyed by offsets. Empty text and zero lines remain distinguishable. | Paged delivery reads exist; Workbench text still has a separate whole-file path. | Use versioned 5,000-line / 2 MiB pages with exact separators, EOF and empty-file handling. |
| More text | Button and reaching the scroll boundary request one next page; existing pages stay visible while reading. | Delivery paging exists, with component-local state. | One next-page operation per content owner; retain the prefix and show inline progress. |
| Refresh during a read | Refresh is enabled. Reset advances the generation, discards content and prevents old settlements from publishing. | Initial-read refresh is disabled; refresh during More can collide with the lease's busy flag. | Record and coalesce the newest intent synchronously, retire old publication, and start the replacement only after physical settlement. This intentionally strengthens DSH's face-level restart to satisfy native ownership. |
| Repeated refresh | The newest content revision owns publication. | Re-render/effect retries do not form a physical-settlement barrier. | At most one active operation plus one latest replacement intent; no overlapping read or premature permit release. |
| Text version change | A different-version page after line 1 is discarded and the walk restarts from line 1. | Restart exists but resets scroll and can race the old request. | Discard the complete old prefix, restart once after settlement, and retain only inert view metadata across versions. Never concatenate or restore payloads of different versions. |
| Body hide / return | Tab store outlives body mounts and retains pages/complete bytes; unchanged return reads no content again. | Active lease disposal drops results; return rereads the saved prefix. | Preserve bounded, versioned results outside mounted renderers. Reauthorize/validate freshness before reuse. Readers, workers, canvases and URLs still retire when hidden. |
| Retention and eviction | Tab pages/complete bytes live until tab abort; Office results have an 8-entry / 64 MiB browser cache. Resource-address records themselves are not bounded. | No retained content cache. | Bound inactive results to 8 entries / 64 MiB with LRU disposal, and bound active text prefixes. Eviction, explicit refresh, version change, close, and authority retirement reclaim payloads. Eviction is an explicit reason to reread. |
| Refresh scroll | Store reset keeps scroll, wrap and answered navigation; the rebuilt body restores the saved position. | Version restart explicitly writes zero scroll offsets. | Preserve scroll targets across refresh and version restart; loading/layout clamping must not overwrite the retained target. User scrolling remains authoritative. |
| Line navigation | Loads enough pages for an explicit line and answers a navigation revision once. | Workbench code approximates line position; delivered previews have no viewer-wide contract. | Keep line navigation separate from remount restoration; do not replay a consumed navigation on return. |
| Automatic refresh | `autoRefresh` starts true. The pause control is hidden in the pinned UI. Changes refresh a mounted, non-loading preview; inactive tabs become dirty. | No file-resource observation. A later page can discover a version mismatch. | Automatic refresh is on by default and driven by actual target observations. A dirty inactive entry is invalidated and refreshed when visible. Later-page mismatch is a separate consistency fence, not the notification mechanism. |
| Change subscription | One shared feed per Session/path; watch acknowledgement precedes stat, queued/live changes are filtered to the bound Host target, and equal versions are suppressed. Reconnection restats once. | Product Host has terminal observation and configuration notifications, but no committed-file metadata/change feed. | Add a narrow authenticated metadata observation path through the existing native delivery authority. Establish observation before the initial snapshot; coalesce hints, verify descriptors/versions, filter unrelated targets and publish only changed metadata. No polling loop. |
| Close / switch / permission change | Tab abort forgets its store and resources; a closed/reopened tab starts fresh. | Logical occurrences are already monotonic; reads retire on scope changes, but new retained data needs the same fences. | Close purges results and waits for retiring physical work before a successor acquires capacity. Scope/permission retirement clears all incompatible payloads synchronously and late work cannot recreate them. |
| Binary loading | Whole bounded bytes, not progressive PDF/range fetching, are retained by the tab. | Binary loading and multipart transport exist, but results are component-local and the source version is discarded at the browser read boundary. | Retain complete typed results with their source version; reuse only within the exact authorized identity. Refresh still serializes on true settlement. |
| Office conversion | Renderer-owned Host conversion; browser cache reauthorizes the source, verifies version and Host renderer generation before reuse. | Secure Host conversion exists; body remount reconverts and refresh can collide with derivation. | Retain bounded converted results per native source/version/Host authority, reauthorize before reuse, and serialize refresh/cancellation on converter settlement. Preserve the native one-converter limit and snapshot/digest checks. |
| PDF / image view | Versioned complete input; tab-owned zoom/reading metadata; inactive runtimes unmount. | PDF state is retained; delivered images lack the Workbench zoom behavior. | Align view controls and scroll/zoom metadata without retaining inactive runtime owners or bypassing raster/pixel admission. |
| Spreadsheet | XLSX/XLS/CSV/TSV use the shared browser reader; selected sheet and errors belong to the tab/renderer. | Delivered XLSX uses it; other entries and formats differ. | Align safe supported formats and selected-sheet restoration. Keep pre-parser ZIP/cell admission; unsupported legacy binary formats remain explicitly unsupported until equally bounded admission exists. |
| Markdown | Growing text pages, frontmatter presentation, source/code alternatives and resource dependency tracking. | Delivery Markdown differs from Workbench presentation and has no dependency feed. | Reuse existing safe Markdown/code presentation. Relative resources require their own existing native authority; a reference in Markdown must not grant a new file read. |
| HTML | Basic sanitized rendering; optional interactive packed documents with `sandbox="allow-scripts"` and related-file reads. | Delivered HTML uses a stricter opaque, script/resource-free policy and a source mode. | Align safe viewer selection/loading/state; retain rustX's strict HTML boundary. Interactive scripts, arbitrary related reads and broader CSP are intentionally not migrated. |
| Errors and retry | Initial failure chooses retry/open/none by failure kind. Later-page failure keeps the prefix and retries the failed page. | Generic retry restarts all content, and some failures render raw errors. | Localize supported native failures. Retain and retry a failed next page; Refresh remains a distinct reset. Permission failures clear protected data. Capacity/unknown settlement do not trigger an automatic retry loop. |
| Empty and loading | First-read/renderer progress, inline More progress, explicit unsupported/empty/failure states. | Loading/error precedence differs between generic, Office and Workbench bodies. | Define one explicit state contract and verify visible state alongside actual operation counts. |
| External actions | Host open/reveal contributions can use broad Session/path authority. | Delivery original Download and Workbench native Open have narrower, separate authorities. | Preserve those actions and their authority. Do not infer an absolute Host path or add DSH's broad filesystem/plugin permissions. |

## Native invariants

The following remain requirements, not optional compatibility behavior:

- Committed delivery coordinates, namespace, original root mapping, attachment/client generation, Host authority and live Child ownership remain part of authorization and cache identity.
- Metadata/freshness checks use the native descriptor walk and authority checks. An observation or cache key cannot grant a read.
- The existing two-read, one-derivation, two-visible-body and URL/worker limits remain. Cancellation retires publication immediately; capacity follows actual terminal settlement.
- No payload is reused across files, source versions or permission scopes. Scroll/view metadata alone may survive a content refresh of the same occurrence.
- Observation has finite admission and deterministic teardown. Unsupported observation is an explicit unavailable state with manual refresh; it does not silently become periodic polling.
- Native public App Server/schema boundaries remain; any observation extension stays in the existing authenticated Product Host/delivery lane.

## Verification contract

Use controllable native/transport promises, gates and observer acknowledgements. Assert UI state and content-read, metadata-read, active-operation, URL, worker and close counts.

Required cases: initial/read-More refresh; repeated refresh; stale success/failure/finally; refresh while a retired read is still physically alive; source/version mismatch; close/reopen; tab/Session/renderer switch; unchanged-cache return; bounded LRU eviction; preserved scroll and new user input; exact resource changes versus unrelated and duplicate notifications; observation reconnect; permission/Host/attachment retirement; Office refresh and cached result reuse; failed-next-page retry; unsupported/empty state; and physical capacity release.

## Implementation and regression evidence

`PreviewContentOwner` owns versioned results, one physical operation, and one
coalesced successor intent. Delivery and Workbench adapters use that owner while
preserving their separate native authority. One App cache retains at most eight
inactive results / 64 MiB. Body unmount releases URLs, workers and canvases;
explicit close, ownership retirement, changed version and eviction purge payloads.
Workbench retains at most four exact scopes and eight file tabs per scope.

Workspace read failures preserve explicit permission/source/version kinds through
the Host carrier. A known denial, missing or non-regular source removes the
loaded prefix immediately, even before a later change hint. A version change
during a read restarts at line 1 after physical settlement; it never appends a
mixed-version suffix or waits for a future page request to discover the change.

The paired Host's exact-path/lookup-edge filesystem subscription precedes the
authorized initial snapshot. Committed deliveries additionally use a private
native metadata/ownership subscription under the existing Product Host credential.
All targets share one bounded browser event stream per Host scope; individual
initial probes retain settled acknowledgements and individual cancellation/end
records. Inactive watches cannot exhaust the HTTP/1.1 connection pool and block
content reads. Backpressure coalesces hints and has a finite terminal-record
budget; overflow retires the channel rather than growing a queue.
Workbench ownership hints are filtered by Session. Every hint invalidates active
publication immediately; a version probe starts only after old physical reads
settle. Equal versions restore the existing prefix without content rereads.
Observer loss retires protected results; explicit Retry establishes a new
subscription rather than starting a reconnect/polling loop.

| Regression suite | Assertions |
| --- | --- |
| `preview-content`, `session-files` | Initial/More refresh, repeated intent, late success/error, same-version More resumption, exact failed-offset Retry, actual active-read counts, version restart, immediate Workspace/native authority-error purge before independent hints, no stale result after close/reopen, Office settlement/cache, bounded LRU |
| `preview-view-state`, `workbench-retention`, `preview-workspace` | Scroll/zoom/sheet/viewer state, layout clamp versus user intent, cache return masked until authorization, new line demands on cached tabs, non-EOF prefix restoration after eviction, consumed navigation without duplicate reads, Session return without content reread, attachment/scope retirement and body limits |
| `file-observation`, `file-observation-native` | Ready-before-snapshot, actual atomic replacement notifications, unrelated filename/Session suppression, ancestor replacement and physical watcher close, native error-without-close settlement while joining other live handles, reusable Host admission and shutdown, exact text/EOF handling |
| `preview-dependencies`, `html-view-lifetime` | Serialized related-resource reads/probes, duplicate-version suppression, actual dependency change, late asset denial, explicit capacity failure and teardown |
| `document-http-lifetime`, `product-host-file-read`, native `protocol::delivery` | Omitted/null/malformed observation coordinates rejected before acknowledgement or socket admission with subsequent reads usable, bounded serialization/send retirement, real HTTP/native cancellation before/after acknowledgement, held physical read and watcher cleanup, exact cancellation tokens/scope, private authentication, native read/observer permit return |
| `xlsx-bounds`, `xlsx-archive` | Sparse/encoded/invalid/duplicate sheet IDs cannot reach ExcelJS allocation, ordinary merges/styles/cached formulas, bounded ZIP/cell/range/name admission and original-byte Download |
| Browser `file-delivery`, `preview-workspace`, `workbench-documents`, `workbench-file-links`, `workbench-office`, `documents` | Real native/Host/browser entry points, supported viewer routing, loaded-prefix/scroll/cache restoration, original downloads, worker/URL retirement, safe HTML and Office rendering |

These regressions use controlled promises, observer acknowledgements, native
read gates and real filesystem events. They do not use longer sleeps or weakened
resource assertions. Commands: `cargo fmt --all --check`, native scripted delivery
boundaries, `cargo clippy --lib -- -D warnings`, Web `typecheck`, `test`,
`check:provenance`, `check:i18n`, production build and the pinned Linux browser
suites. The [PR checks](https://github.com/Caismis/rustX/pull/505/checks) carry
the current-head FULL verdict, including complete native/platform boundaries;
historical runs are not evidence for a newer head.

## Explicit remaining differences

- Native delivery identity is stricter than DSH Session/path identity. A return
  performs an authorized one-byte metadata probe before exposing retained mutable
  content. Equal versions require no page/full-byte read or reconversion.
- Delivery HTML remains script/style/resource free in an opaque sandbox. Existing
  Workbench interactive HTML remains an explicit opt-in under its existing
  workspace authority. A delivered Markdown reference grants no related-file
  access. DSH's broad resource/plugin/permission model is not migrated.
- SVG is source/code only; ICO and legacy DOC/PPT/XLS remain unsupported. OOXML,
  raster and spreadsheet admission is retained rather than admitting formats
  without equivalent bounded pre-parser checks. CSV/TSV and safe text viewer
  alternatives share the supported spreadsheet/text presentation.
  Native typed managed images retain their image/MIME classification even with
  extensionless display names; SVG/ICO payloads still fail raster admission.
- DSH resource metadata is not globally bounded. rustX retains eight inactive
  results / 64 MiB and 32 shared Host target observations (with at most 128 lookup
  edges each); native ownership observers also cap at 32. Workbench admits 64
  dependency declarations but the shared observation budget can reject earlier.
  The localized capacity state is explicit and does not loop/retry automatically.
- DSH reconnects its resource feed and restats. rustX clears protected data on
  observation loss and requires explicit Retry/new authority; uncertain physical
  settlement closes the affected admission domain. Hosts without observation
  display a manual-refresh notice and cannot retain mutable payloads on hide.
  The paired delivery observer requires the operator's shared-filesystem
  attestation (set by the local launcher); without it observation/preview setup
  fails closed instead of watching unproven Host paths. Original Download still
  uses its separate native authorization.
- The closed native `SessionFileReadFailure::ReadFailed` does not distinguish
  invalid UTF-8 from other I/O failures. That case keeps a localized generic Retry
  and original Download rather than inventing a wire failure or changing the
  public protocol. Known missing/unavailable/revoked and initial size failures
  suppress Retry; header Refresh and the existing original action remain.
- The existing inert HTML source inspector remains available in addition to safe
  viewer choices. Native PDF/page/pixel, source-byte, parser, converter and
  physical-retirement bounds continue to take precedence over DSH fidelity.
