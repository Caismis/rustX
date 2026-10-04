# Advanced document previews

This is the advanced-viewer companion to [file delivery](file-delivery.md).
The original two source owners, committed delivery identity, historical
Conversation mapping, descriptor-open policy and original downloads remain
normative there. A document suffix selects presentation; it never grants access.

## Capabilities

All views are read-only. No viewer calls a model or activates an Agent. No
document registry, persisted view preferences, conversion cache or background
conversion exists. Page, zoom, sheet and HTML mode belong to the logical tab
occurrence in the bounded in-memory Session preview workspace. Inactive tabs
retain metadata only; their document runtimes are unmounted.

| Extensions | Sources | Presentation | Runtime | Passwords | Fidelity |
| --- | --- | --- | --- | --- | --- |
| `.pdf` | Artifact, Session file | PDF.js canvas and selectable text, one selected page | Browser module Worker | Rejected, original download retained | No scripting, XFA, interactive forms, annotations or embedded attachments; no OCR |
| `.docx`, `.pptx` | Artifact, Session file | Product Host converts a private authorized snapshot to PDF, then the same PDF viewer | Linux with `/usr/bin/bwrap`, `prlimit`, system LibreOffice, fonts, working user/network/PID namespaces and a systemd user manager with cgroup v2 CPU/memory/pids controllers | Encrypted ZIP/OOXML rejected | Fonts, layout and pagination can differ; a visible derived-preview warning is mandatory |
| `.xlsx` | Artifact, Session file | Product Host parser produces a bounded sheet/cell inspection model | Node 24 Product Host, Linux or macOS | Encrypted packages rejected | Stored values and literal formulas only; no formatting, charts, images, merged-cell layout, calculation or editing |
| `.html`, `.htm` | Artifact, Session file | Opaque-origin sandbox plus inert UTF-8 source inspection | Browser | Not applicable | External resources, active content, links and forms removed/blocked |

Legacy `.doc`, `.ppt`, `.xls`, `.xlsm`, CSV and arbitrary archives have no new
viewer. The basic Markdown/text/code/raster viewers retain their existing policy.
macOS Office conversion is unavailable: there is no unsandboxed fallback.
Missing Linux sandbox/converter prerequisites fail closed. No runtime binary
download is performed. The operator installs and maintains LibreOffice and fonts.

Every Download remains the original authorized source bytes and original name,
including Unicode/spaces. A generated PDF or workbook projection never replaces
that source. Conversion/parser failures preserve the original Download action,
which reauthorizes when invoked; read failures allocate no download URL. Authority retirement
retires every incompatible active occurrence lease, including its original URL.

## Exact budgets

| Resource | Limit |
| --- | --- |
| Original Artifact bytes | 256 KiB, existing owner policy |
| Original Session-file bytes | 512 KiB, existing owner policy |
| Preview original transfers / URLs | 2 active transfers; 2 visible occurrence URLs plus 1 transient Download URL (3 aggregate) |
| Inline Conversation Artifact transfers / retained URLs | Existing separate 2 / 16 owner, unchanged |
| Active preview bodies / browser derivation demand | 2 visible bodies; 1 active Host derivation with at most 2 visible intents waiting during retirement |
| Active derived operations per Product Host | 1; browser serializes visible demand, no automatic retries |
| Retained derived cache entries | 0 |
| Derived PDF / admitted PDF data | 4 MiB |
| PDF pages | 100 |
| Browser PDF workers / active renders / presentation canvases | 2 / 2 / 2 aggregate; 1 of each per visible PDF-backed occurrence |
| Each page or scratch canvas | 4096 per side / 4,194,304 pixels; checked before allocation |
| PDF.js scratch canvases | At most 8 per document (16 aggregate), 16,777,216 pixels per document; bounded factory rejects before create/reset |
| Total admitted canvas pixels | 20,971,520 page + scratch per document (41,943,040 for two); plus one shared 300 × 150 text-measurement canvas (normalized null language); hardware/OffscreenCanvas/image decoders disabled |
| PDF decoded image policy | 4,194,304 pixels; larger images are omitted by PDF.js |
| PDF text layer | 10,000 items, 100,000 characters, current page only |
| PDF load / selected-page cooperative watchdog | 15 seconds each; when the browser event loop dispatches expiry, retire the worker |
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
| Office service runtime / aggregate CPU rate | 15 seconds (external systemd timer); one CPU quota, 100 ms per 100 ms period across the complete cgroup |
| Converter writable filesystem | One 64 MiB private tmpfs at `/tmp`, including `/dev/shm`; root and input read-only. Private Host snapshot ≤512 KiB plus fixed profile seed |
| Converter aggregate memory / swap / tasks | 512 MiB cgroup memory, zero swap, 64 tasks including threads and descendants; whole-cgroup OOM kill |
| Converter per-file / per-process FD limits | 8 MiB / 128 (supplementary; not aggregate limits) |
| Converter stdout accepted as PDF | 4 MiB, overflow kills the sandbox |

Original source buffers are retained only by the selected tab in each visible pane
and its original Blob. Each derived PDF has one occurrence-owned byte buffer plus
its transferred worker copy. Two visible PDF-backed documents are valid, including
PDF + PDF or converted Office + PDF. Hidden tabs own zero workers, canvases,
original URLs, derived buffers or derivation demand. HTML has no additional resource
URLs or persistent derived cache. A transient Download reauthorizes original bytes
and revokes its separate URL exactly once after dispatching the browser download.

V8 heap limits do not include external ArrayBuffers; ZIP expansion and source
buffers have independent byte bounds. PDF.js internal decoding allocations are
not a browser-enforceable process heap limit. The canvas, image, page, text and
owner-retention limits above must not be described as a hard cap on browser RSS.
PDF main-thread timers are cooperative watchdogs, not hard CPU/preemption
limits. Synchronous render/text work can delay their dispatch. Worker termination
retires worker execution once requested; it cannot preempt synchronous browser
main-thread work. This deliberately differs from the external Office service timer.
PDF fonts use the built-in glyph-path renderer (`disableFontFace`); previews do not register document FontFaces in application state. Text selection uses a fallback font layer and can differ in glyph appearance. Public PDF filter resources, selected-page buffers and owner references are released explicitly even when a failed worker cannot acknowledge PDF.js destruction.

Oversized sheet coordinates/cell counts produce a visibly truncated inspection.
Other structural, string, sheet-count or model-size limits reject the projection.
ZIP64, streaming data descriptors, explicit directory entries, unindexed ZIP payloads, split/encrypted archives, ambiguous/duplicate/path-conflicting names,
traversal, absolute paths, nested packages and malformed headers are rejected.
All central and local headers are checked before any inflation. Each inflate
also has an actual output cap, followed by size/CRC validation. No ZIP entry is
ever extracted as a filesystem path.

## Source and lifecycle ownership

`FilePreviewCoordinator` coordinates original reads and Host admission for one
compatible Session/runtime/authority/target scope. Each `FilePreviewLease` is
bound to one exact source and occurrence and owns its cancellation signal,
original URL and active derivation demand. The logical workspace never holds bytes
or URLs. The coordinator admits at most two visible leases and one independent
transient Download. Original read admission stays at two with at most three
current intents waiting during settlement; no native transfer bound changes.
The one active derivation reserves one of those two read permits for its complete
lifetime, because its internal reauthorization uses the same Host/native read
budget. This leaves one original pane/Download transfer while conversion runs. The browser sends only an attachment
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

Tab selection, hide/collapse, unmount, reconnect, attachment or authority
replacement retire the active lease before stale completion can publish. The lease
checks client generation, Session, authority revision, exact target, source and
its own active identity. Close/reopen creates another occurrence and lease; source
equality does not revive retired work. Abort fences stale successes, failures and
loading completion.
Host retirement aborts parser/converter work. Its slot is held until the worker
terminates and the process pipes close and temporary directory cleanup completes.
There are no automatic retries or unknown-outcome conversion joins.

A small browser admission primitive coordinates the two visible derivation
intents. A waiting occurrence leaves immediately on abort. An active operation
keeps admission until Host settlement, including after Session replacement. The
HTTP carrier acknowledges an exact ephemeral operation token in response headers
before accepting cancellation. Cancellation sends that token and its exact Host
scope on a separate request; the original terminal response stays open until
physical cleanup completes. Thus aborting a browser fetch is never mistaken for
Host settlement. The Host retains only its single active cancellation handle, no
tab registry, queue, document cache or persisted operation. Unknown settlement
fails closed for that Host authority; it never starts a competing conversion.
The private native file-read socket also waits for its admitted read to retire
before acknowledging a clean close. This repairs an existing cancellation gap:
dropping the transport's receiver left its detached native descriptor read and
permit alive. The Node Host waits for that exact close acknowledgement or a
terminal read response; an abnormal close leaves admission unavailable. Office
source-read uncertainty also retains its document slot. Browser-only coordination
cannot prove retirement of that native permit. No public App Server Method,
schema, authority or filesystem policy changes.

## Office boundary

The Host preflights OOXML in a Node worker before starting LibreOffice. VBA,
ActiveX, embeddings, external links/relationships and DTDs are rejected. Only
DOCX and PPTX are submitted to the converter. A fresh private profile sets macro
security to level 3. This setting alone is not the sandbox boundary.

`office-sandbox.ts` starts one transient **user** systemd service, without a
persistent rustX worker or a new daemon. Its unique unit owns Bubblewrap and every
descendant in one cgroup v2 domain before document-controlled execution begins.
MemoryMax=512 MiB, MemorySwapMax=0, TasksMax=64 and CPUQuota=100% apply to the
whole tree. OOMPolicy=kill sets memory.oom.group=1. These are not UID-scoped
RLIMIT_NPROC or per-child RLIMIT_AS limits. Before releasing the trusted stdin admission gate, the Host reads the
actual service cgroup's memory.max, memory.swap.max, pids.max, cpu.max and
memory.oom.group and requires the exact values before accepting a stdin permit.
Missing controllers, unsupported properties or an unavailable manager fail closed
as `converter_unavailable`; no conversion payload is admitted.

The operator must provide `/usr/bin/systemd-run`, a reachable
systemd user manager and delegated cgroup v2 CPU/memory/pids controllers. The Host
uses only XDG_RUNTIME_DIR/DBUS_SESSION_BUS_ADDRESS to contact that manager.
These are client control-plane inputs, not payload variables. The already-running
user manager can independently supply its own imported environment to the
transient service; that inheritance is not trusted for secret isolation.

After the existing cgroup admission permit, the trusted shell execs
`/usr/bin/env -i /usr/bin/bwrap`. The external exec constructs an **empty initial
Bubblewrap environment**, replacing the admission shell's address space before
Bubblewrap starts. Neither the shell nor env remains as an ancestor inside the
sandbox PID namespace. `/usr/bin/env` is a required local runtime prerequisite;
there is no alternate path when it is missing.

Bubblewrap's `--clearenv` remains defense in depth only. It is not relied upon to
erase inherited strings from kernel-visible `/proc/PID/environ`: this deliberately
avoids depending on upstream [issue #725](https://github.com/containers/bubblewrap/issues/725)
or its proposed [fix #800](https://github.com/containers/bubblewrap/pull/800).
The Host supplies exactly `PATH=/usr/bin`, `HOME=/tmp/home`, `LANG=C.UTF-8`,
`SAL_USE_VCLPLUGIN=svp`, and `TMPDIR=/tmp` through explicit Bubblewrap `--setenv`
arguments. Bubblewrap's `--chdir /tmp` additionally generates `PWD=/tmp`.
The fixed conversion shell may maintain its own shell bookkeeping; no Host or
manager variables are forwarded. There is no configurable environment passthrough.

A real Linux regression sets a unique synthetic secret through
`systemctl --user set-environment` and first proves a separate user service
inherits it despite a restricted systemd-run client environment. The production
sandbox must then expose an empty `/proc/1/environ`, and precisely the six values
above in `/proc/self/environ` and the effective payload environment. It also
inspects every visible process environment. Assertions run inside the probe so
failure diagnostics never print inherited secrets. Finally, awaited
`unset-environment` and a fresh service prove removal of the test's unique key.
A separate harmless probe records whether the installed Bubblewrap retains a
synthetic initial secret with `--clearenv` alone; production assertions do not
depend on whether an upstream release has fixed that behavior.

The Host never changes global cgroup policy or starts a privileged conversion service.
CI provisions the runner's user manager with loginctl enable-linger and verifies
the actual production admission path before tests. macOS remains unsupported for
Office; there is no alternate conversion mode.

Bubblewrap unshares user, network, PID, IPC and UTS namespaces, disables further
user namespaces, drops capabilities, creates a new session and uses die-with-parent.
The root filesystem and procfs are remounted read-only. `/usr` and the private
input/profile seed are read-only. The only writable filesystem is the size-limited
`/tmp`; `/dev/shm` is a symlink into it. `/dev` itself is on the read-only root;
only null, zero, random and urandom devices are individually bound, with no extra
dev tmpfs or devpts mount. Tmpfs data is bounded to 64 MiB and its memory/metadata
is also charged to the operation cgroup. No home, Workspace, runtime store, socket
directory or credentials are mounted. Payload environment is cleared and explicitly
supplied, with no proxy/token/desktop variables. The network namespace has no Host
connection. prlimit supplies only supplementary file-size/FD/core-dump limits.
The exact fixed boundary lives in `web-console/host/documents/office-sandbox.ts`.

`/etc` is empty except for a read-only `/etc/libreoffice/registry` mount when
that system directory exists. Debian/Ubuntu's `/usr` package symlinks require
this registry; distributions storing it under `/usr` need no extra mount.
No other Host `/etc` configuration is exposed. This does not relax namespace,
macro, temporary-disk, process or network restrictions.

Ubuntu 24.04 also requires its packaged AppArmor Bubblewrap profile to be
loaded: install `apparmor-profiles`, then have the operator load
`/usr/share/apparmor/extra-profiles/bwrap-userns-restrict` with
`apparmor_parser -r`. This grants namespace-setup capabilities to
`/usr/bin/bwrap` and denies capabilities to its executed children. CI provisions
that profile, separately asserts that
`kernel.apparmor_restrict_unprivileged_userns` equals `1`, and checks actual
Bubblewrap namespace admission before running real conversion. rustX does
not change AppArmor policy, run conversion as root, share the Host network or
fall back to an unsandboxed converter if the prerequisite is unavailable.

Conversion writes only into the private tmpfs. Its PDF is streamed over stdout,
bounded, and checked for a PDF signature before the PDF viewer validates it.
Before permitting payload execution, the Host opens the actual cgroup.kill and
watches cgroup.events outside the sandbox. Cancellation/oversize uses cgroup.kill
to SIGKILL the complete tree. No process can migrate out because the sandbox has
no cgroup mount or inherited control descriptor.
RuntimeMaxSec=15 is enforced by the external manager, even if the Node event loop
is blocked; the Node timer provides an additional cooperative error watchdog.
KillMode=control-group, KillSignal=SIGKILL and ExitType=cgroup prevent a main-process
exit from hiding surviving descendants. A late-starting service cannot receive
its stdin permit after cancellation. The systemd-run wait client is never killed
to simulate settlement: --wait waits for service/cgroup retirement, and its close
also waits for forwarded pipe closure. The Host also kills and awaits kernel populated=0 (or removal of the empty
cgroup) before closing its control handles. Thus even an unexpectedly failed wait
client cannot hide surviving descendants. If retirement cannot be established, the
Host retains its admission slot and reports converter_unavailable; it does not
admit another operation on an unknown outcome.
Only then is the private input directory removed; only after that awaited cleanup
does LocalWorkspaceHost release the one derivation slot. Failed/oversized output
never publishes. --collect retires the transient unit; no derived cache remains.
stderr is discarded rather than exposing converter internals or filesystem paths.

## Document contract ownership

`web-console/shared/documents.ts` owns environment-neutral source/request/result
DTOs and the explicit shared derived-PDF carrier byte bound. Browser canvas/page/
text limits and independent load/render watchdog values live in
`src/client/pdf-limits.ts`. Host workbook/parser/converter limits live in
`host/documents/limits.ts`; OOXML admission limits remain with the Host archive
owner. Host code does not import document policies from the browser client.
`shared/session-file-identity.ts` compares each typed Session-file identity field
including the original scope, path/name/MIME and normalized optional description.
This is only stale-source detection; equality never grants read authorization.

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
| `dompurify` | 3.4.16 / Apache-2.0 OR MPL-2.0 | Browser HTML defense in depth; does not grant iframe authority |
| `saxes` | 6.0.0 / ISC | Node worker, strict XML events; no DTD/entity/network loader or execution engine |
| Node `zlib` | Node 24 runtime | Host-only bounded raw inflate and CRC; no archive-path extraction |
| LibreOffice | Tested 26.2.6.3; MPL-2.0/LGPL-3.0 with bundled-component licenses | Operator-installed Linux converter; expands admitted OOXML inside the sandbox; macros disabled/rejected; network namespace isolated |
| systemd | Tested 259.9 locally; requires ExitType=cgroup support and cgroup v2 delegation; LGPL-2.1-or-later core | Transient service cgroup ownership, aggregate controls, external runtime deadline and settlement; no document parsing |
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
LibreOffice 26.2.6.3, Bubblewrap 0.12.0 and util-linux 2.41.5. Real DOCX/PPTX
conversion is also exercised with Ubuntu 24.04's LibreOffice 24.2.7 and
Bubblewrap 0.9.0 package layout, including its external system registry.

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
- `test/document-http-lifetime.test.ts` gates real HTTP header acknowledgement,
  exact cancellation and physical settlement; cancellation-before-ACK and wrong
  scope/token cannot race Host capacity.
- `test/pdf-document.test.ts`, `test/document-view-lifetime.test.tsx` and
  `test/session-files.test.tsx` cover worker/render limits, backing-store cleanup,
  obsolete results/errors and existing source/attachment/authority fences.
- `test/e2e/documents.spec.ts` exercises the actual native/Host/browser path,
  original download bytes, PDF selection/navigation/zoom and worker recovery,
  XLSX cached formulas, hostile HTML effects, localized/narrow/theme views and
  stable reading during a gated live response. Existing basic file-delivery,
  geometry, native root/leaf replacement and historical ownership suites remain
  part of the full validation lane.

The aggregate boundary regressions execute controlled Python helpers in the actual
production sandbox: writes outside `/tmp` fail, `/dev/shm` shares its quota,
concurrent descendants hit pids.max and memory.max, and cancellation, external
runtime expiry (with the Node watchdog clock held), and wait-client failure retire
all cgroup tasks. Gated Host tests keep capacity occupied through process and input
cleanup, reject unknown retirement, and admit the next operation only after success.
The native Artifact browser seam seeds a real XLSX through ArtifactStore using a
fixture-only Cargo example, then exercises the actual HTTP Host, private v2 socket,
native Artifact read owner, digest/reauthorization, workbook publication and original
download. Held publication is rejected after native attachment replacement; no
model request is made. No product Artifact write API is introduced.
