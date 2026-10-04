# Advanced document previews

This is the advanced-viewer companion to [file delivery](file-delivery.md).
The original two source owners, committed delivery identity, historical
Conversation mapping, descriptor-open policy and original downloads remain
normative there. A document suffix selects presentation; it never grants access.

## Capabilities

All views are read-only. No viewer calls a model or activates an Agent. No
document registry, persisted view preferences, conversion cache or background
conversion exists. Page, zoom, sheet and HTML mode belong to the selected view.

| Extensions | Sources | Presentation | Runtime | Passwords | Fidelity |
| --- | --- | --- | --- | --- | --- |
| `.pdf` | Artifact, Session file | PDF.js canvas and selectable text, one selected page | Browser module Worker | Rejected, original download retained | No scripting, XFA, interactive forms, annotations or embedded attachments; no OCR |
| `.docx`, `.pptx` | Artifact, Session file | Product Host converts a private authorized snapshot to PDF, then the same PDF viewer | Linux with `/usr/bin/bwrap`, `prlimit`, system LibreOffice, fonts and working user/network/PID namespaces | Encrypted ZIP/OOXML rejected | Fonts, layout and pagination can differ; a visible derived-preview warning is mandatory |
| `.xlsx` | Artifact, Session file | Product Host parser produces a bounded sheet/cell inspection model | Node 24 Product Host, Linux or macOS | Encrypted packages rejected | Stored values and literal formulas only; no formatting, charts, images, merged-cell layout, calculation or editing |
| `.html`, `.htm` | Artifact, Session file | Opaque-origin sandbox plus inert UTF-8 source inspection | Browser | Not applicable | External resources, active content, links and forms removed/blocked |

Legacy `.doc`, `.ppt`, `.xls`, `.xlsm`, CSV and arbitrary archives have no new
viewer. The basic Markdown/text/code/raster viewers retain their existing policy.
macOS Office conversion is unavailable: there is no unsandboxed fallback.
Missing Linux sandbox/converter prerequisites fail closed. No runtime binary
download is performed. The operator installs and maintains LibreOffice and fonts.

Every Download remains the original authorized source bytes and original name,
including Unicode/spaces. A generated PDF or workbook projection never replaces
that source. Conversion/parser failures preserve an already authorized original
download; read failures allocate no download URL. Existing authority retirement
disposes the complete selected preview, including its original URL.

## Exact budgets

| Resource | Limit |
| --- | --- |
| Original Artifact bytes | 256 KiB, existing owner policy |
| Original Session-file bytes | 512 KiB, existing owner policy |
| Session-file transfers / retained URLs | 2 / 2, unchanged |
| Artifact transfers / retained URLs | 2 / 16, unchanged |
| Active derived operations per Product Host | 1; capacity rejects, no queue/retry loop |
| Retained derived cache entries | 0 |
| Derived PDF / admitted PDF data | 4 MiB |
| PDF pages | 100 |
| Browser PDF workers / active renders / presentation canvases | 1 / 1 / 1 |
| Each page or scratch canvas | 4096 per side / 4,194,304 pixels; checked before allocation |
| PDF.js scratch canvases | At most 8, aggregate 16,777,216 pixels; bounded factory rejects before create/reset |
| Total admitted canvas pixels | 20,971,520 page + scratch, plus one 300 × 150 text-measurement canvas; hardware/OffscreenCanvas/image decoders disabled |
| PDF decoded image policy | 4,194,304 pixels; larger images are omitted by PDF.js |
| PDF text layer | 10,000 items, 100,000 characters, current page only |
| PDF parse / selected-page deadline | 15 seconds each; expiry retires the worker |
| ZIP entries / path bytes | 256 / 240 |
| ZIP individual / total expansion | 4 MiB / 16 MiB |
| ZIP declared expansion ratio | 200:1 |
| Node parser worker | 1, 15 seconds; V8 old/young heap 64/16 MiB, stack 2 MiB |
| Workbook sheets / rows / columns | 16 / 2000 / 128 |
| Workbook admitted cells | 20,000 across all sheets |
| Shared strings | 20,000, aggregate 1,048,576 characters |
| Cell text or formula | 4096 characters |
| XML depth / elements per part | 64 / 200,000 |
| Serialized workbook model | 2 MiB |
| Mounted spreadsheet cells | 100 per inspection window |
| Office conversion wall / CPU deadline | 15 seconds / 15 CPU seconds |
| Converter writable filesystem | 64 MiB private tmpfs; original snapshot ≤512 KiB in private Host directory |
| Converter address-space / output-file / open-FD limits | 1 GiB per process / 8 MiB per file / 128 |
| Converter stdout accepted as PDF | 4 MiB, overflow kills the sandbox |

Original source buffers are retained only by the selected view and its original Blob; a derived PDF has one selected-view byte buffer plus its transferred worker copy. HTML has no resource URLs or persistent derived cache.

V8 heap limits do not include external ArrayBuffers; ZIP expansion and source
buffers have independent byte bounds. PDF.js internal decoding allocations are
not a browser-enforceable process heap limit. The canvas, image, page, text and
owner-retention limits above must not be described as a hard cap on browser RSS.
PDF fonts use the built-in glyph-path renderer (`disableFontFace`); previews do not register document FontFaces in application state. Text selection uses a fallback font layer and can differ in glyph appearance. Public PDF filter resources, selected-page buffers and owner references are released explicitly even when a failed worker cannot acknowledge PDF.js destruction.

Oversized sheet coordinates/cell counts produce a visibly truncated inspection.
Other structural, string, sheet-count or model-size limits reject the projection.
ZIP64, streaming data descriptors, explicit directory entries, unindexed ZIP payloads, split/encrypted archives, ambiguous/duplicate/path-conflicting names,
traversal, absolute paths, nested packages and malformed headers are rejected.
All central and local headers are checked before any inflation. Each inflate
also has an actual output cap, followed by size/CRC validation. No ZIP entry is
ever extracted as a filesystem path.

## Source and lifecycle ownership

`FilePreviewResources` owns original byte reads, identity verification, original
URLs and the selected derivation request. The browser sends only an attachment
target, a closed Artifact or committed-delivery coordinate, an extension and the
SHA-256 of its authorized original bytes. It never uploads raw Office bytes or
supplies a Host pathname. The Host authenticates to the private native read seam
with its own secret; the browser cannot use that credential.

Private carrier v2 distinguishes an Artifact ID from a delivery coordinate.
Artifacts still use the original `artifact_read` owner. Session files still use
the original descriptor-open, root and historical mapping owner. Ordinary App
Server schemas remain unchanged; private v1 payloads are not accepted.

The Host reserves its one operation before reading. It reauthorizes and compares
the exact digest before parsing, again after parsing before conversion, and after
conversion before publication. The delivery identity must also remain equal.
There is no path-keyed cache and no result that can bypass a fresh authorization.
An operation consumes an immutable private snapshot while the original Session
file retains mutable-reopen semantics. Converter updates need no cache migration.

Selection, unmount, reconnect, attachment or authority replacement retire the
existing owner. Abort fences stale successes, failures and loading completion.
Host retirement aborts parser/converter work. Its slot is held until the worker
terminates and the process pipes close and temporary directory cleanup completes.
There are no automatic retries or unknown-outcome conversion joins.

## Office boundary

The Host preflights OOXML in a Node worker before starting LibreOffice. VBA,
ActiveX, embeddings, external links/relationships and DTDs are rejected. Only
DOCX and PPTX are submitted to the converter. A fresh private profile sets macro
security to level 3. This setting alone is not the sandbox boundary.

Bubblewrap unshares user, network, PID, IPC and UTS namespaces, drops capabilities,
creates a new session and uses `--die-with-parent`. It mounts `/usr` read-only,
the private input/profile seed read-only, a new `/proc` and `/dev`, and one
size-limited `/tmp`. It never mounts the user's home, Workspace, runtime store,
socket directory or credentials. Environment variables are explicitly supplied;
proxy/token/desktop variables are not inherited. The network namespace has no
Host network connection. `prlimit` supplies the limits listed above; core dumps
are disabled. The exact fixed argument vector lives in
`web-console/host/documents/converter.ts`.

Conversion writes only into the private tmpfs. Its PDF is streamed over stdout,
bounded, and checked for a PDF signature before the PDF viewer validates it.
Timeout, cancellation and oversize output kill Bubblewrap; destruction of its PID
namespace terminates descendants. Settlement waits for process/pipe close before
removing the private input directory and releasing capacity. stderr is discarded
rather than exposing converter internals or filesystem paths to the browser.

## HTML and spreadsheet semantics

HTML source is rendered as React text. Rendered HTML passes through DOMPurify
with active/navigation-bearing elements and URL attributes removed, then enters
an iframe with `sandbox=""` and `referrerpolicy="no-referrer"`. Neither scripts
nor same-origin permission is granted. A first-head CSP uses `default-src 'none'`,
`script-src 'none'`, `style-src 'none'`, `connect-src 'none'`,
`frame-src 'none'`, `form-action 'none'`, `base-uri 'none'`, and `object-src 'none'`.
User styles and all resource-bearing attributes are removed. Only a finite list of static structural elements is retained.
Sanitization is defense in depth; sandbox origin isolation and CSP remain required.

XLSX formulas are inert strings. A formula and its cached `<v>` are separate
fields and UI columns. Missing cache stays missing. Shared/array formulas are
shown as stored; formulas are never expanded, executed or recalculated. External
relationships are rejected, not fetched. Number/date format interpretation is
not performed: inspection shows the stored representation.

## Dependencies and reference provenance

| Dependency | Exact version / license | Placement and behavior |
| --- | --- | --- |
| `pdfjs-dist` | 6.4.299 / Apache-2.0 | Lazy browser renderer and explicitly owned Vite-emitted local worker; no CDN, XFA or PDF scripting. WASM decoders disabled. Selected upstream text-layer CSS is scoped locally. |
| `dompurify` | 3.4.12 / Apache-2.0 OR MPL-2.0 | Browser HTML defense in depth; does not grant iframe authority |
| `saxes` | 6.0.0 / ISC | Node worker, strict XML events; no DTD/entity/network loader or execution engine |
| Node `zlib` | Node 24 runtime | Host-only bounded raw inflate and CRC; no archive-path extraction |
| LibreOffice | Tested 26.2.6.3; MPL-2.0/LGPL-3.0 with bundled-component licenses | Operator-installed Linux converter; expands admitted OOXML inside the sandbox; macros disabled/rejected; network namespace isolated |
| Bubblewrap | Tested 0.12.0; LGPL-2.1-or-later | Operator-installed Linux namespace/process/filesystem boundary; no document parsing |
| util-linux `prlimit` | Tested 2.41.5; GPL-2.0 family (system package includes BSD/public-domain components) | Operator-installed process resource limiter; no document parsing |

The production install notices include the exact dependency closure. Saxes' npm
package omits its license file; `licenses/saxes-6.0.0.txt` reproduces the license
from its upstream v6.0.0 tag and the notice generator checks that exact version.

The Harness checkout was clean at `477b4f420553e8a52c2fbccc464d7561b239c443` and
was inspected read-only using `git show` at approved pin
`639ed015397290b3745d163aafe02ffee4aa3f84`. Exact inspected paths and hashes are in
`web-console/source-inventory.json`. Adapted ideas are document lifetimes, lazy
PDF page cleanup, authorized bounded conversion and isolated HTML. No Harness
code was copied for the new implementation. Cordis, slots, registries, background
work, caches, `libreoffice-kit`, eager `unzipSync`, spreadsheet engines, legacy
formats and HTML dependency loading were deliberately excluded.

The exact `pdfjs-dist@6.4.299` release-age exception is intentional: the
[upstream release](https://github.com/mozilla/pdf.js/releases/tag/v6.4.299) includes
a CPU-denial-of-service fix in text-field sizing. The exception admits only this
locked version, not a package wildcard. Reviewed local Linux prerequisites were
LibreOffice 26.2.6.3, Bubblewrap 0.12.0 and util-linux 2.41.5.

## Contract regressions

- `test/document-archive.test.ts` and `test/document-security.test.ts` exercise
  real OOXML bytes, synthetic bombs, malformed metadata, macro/active content,
  external relationships and the real parser-worker rejection before conversion.
- `test/document-operation-lifetime.test.ts` and
  `test/document-converter.test.ts` gate physical termination/pipe close and
  use controlled timers, proving settlement, cleanup and admission ordering.
- `test/document-integration.test.ts` runs the real sandbox and real DOCX/PPTX
  bytes on Linux. macOS explicitly checks converter unavailability; it does not
  silently skip an unsandboxed conversion path.
- `test/pdf-document.test.ts`, `test/document-view-lifetime.test.tsx` and
  `test/session-files.test.tsx` cover worker/render limits, backing-store cleanup,
  obsolete results/errors and existing source/attachment/authority fences.
- `test/e2e/documents.spec.ts` exercises the actual native/Host/browser path,
  original download bytes, PDF selection/navigation/zoom and worker recovery,
  XLSX cached formulas, hostile HTML effects, localized/narrow/theme views and
  stable reading during a gated live response. Existing basic file-delivery,
  geometry, native root/leaf replacement and historical ownership suites remain
  part of the full validation lane.
