# DeepSeek Harness file preview research

Research date: 2026-10-22. Scope: opening a file explicitly delivered in a
conversation, the resulting Sidebar preview, and the file-size and transport
policies relevant to rustX's 512 KiB Session-file limit.

The reference is DeepSeek Harness commit
[`687f282e8625c6b8d11ff58211a90524be2e4317`](https://github.com/deepseek-ai/deepseek-harness/commit/687f282e8625c6b8d11ff58211a90524be2e4317)
(the dsh 0.2.1-rc.2 release merge). The workspace-file defaults were also checked
at rustX's existing presentation reference,
[`5badb15009ae1756c3afe0ae0cef1faafc290ccc`](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/api/workspace-files/src/index.ts#L186-L190):
the 2 MiB page/window, 32 MiB complete-file and 5,000-line defaults agree.
These are source defaults; a deployment can configure different values.

## Main finding

DSH separates text pagination, bounded complete-byte reads, and renderer-owned
Office conversion. It does not impose one 512 KiB limit on every delivered file.
Binary results use HTTP multipart with JSON metadata and native bytes, avoiding
whole-file Base64 strings. Ordinary PDF, image, HTML and spreadsheet previews
still acquire a complete bounded file; binary transport does not make these
previews incremental or zero-copy.

## From delivery card to preview

The delivery card's preview handler calls `openFile(file.path)`, opening the
ordinary Session file resource in the Sidebar. Delivery cards do not own a
separate small-file byte decoder. The document owner selects a renderer and one
of three loading modes: `text-pages`, `bytes-complete`, or `renderer`.
See [delivery activation](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-deliverables/src/client/Deliverables.tsx)
and [preview reader composition](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/index.ts#L90-L105).

## Read and rendering budgets

| Path | Default budget | Behavior |
| --- | --- | --- |
| Plain text, Markdown and code | 5,000 lines and 2 MiB per page | Line offset and EOF pagination; no whole-file size cap on this API |
| Optional byte-range read | 2 MiB per window | Byte offset and length; independent of the complete-file cap |
| Complete binary file read | 32 MiB | A larger file fails instead of being truncated |
| Browser spreadsheet parser | 16 MiB source, 250,000 rectangular cells, 15 seconds | A separate Worker/parser budget, stricter than the ordinary byte reader |
| Office conversion | 50 MiB input, 100 MiB PDF output, 60 seconds after converter acquisition | Dedicated Host conversion with two active converters by default |
| Browser Office result cache | 8 PDFs and 64 MiB | Bounds retained PDF buffers, not total rendering/process memory |

Sources: [workspace-file defaults and read methods](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/index.ts#L186-L280),
[spreadsheet and browser-cache defaults](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/config.ts#L29-L40),
and [Office provider defaults](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/document/office-to-pdf/src/index.ts#L74-L84).

### Text pagination

The first mount requests page one. Scrolling to the end or choosing Load more
requests the next page. The owner accumulates the prefix, and Markdown/code
render it as one growing document. A response with a different version after
page one discards the old pages and restarts, rather than joining versions.
See [page loading and version handling](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/face.ts#L146-L164)
and [the preview contract](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/README.md#L54-L58).

The Host cuts the requested lines from a decoded stream, keeping only the page.
It rejects the page immediately when its byte count exceeds the cap; it does
not silently shorten the page or split an oversized line. Consequently a
600 KiB text file can load, but one line exceeding 2 MiB still fails with the
default settings. Later line offsets scan and discard preceding lines. Browser
prefix retention grows as more pages load, so per-page limits are not a fixed
total-memory bound. See [the page cutter](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/index.ts#L107-L159).

### Complete binary files and PDF

The shared preview reader calls `workspaceFiles.readBytes(sessionId, path, {},
signal)`. Empty options select a complete read; a caller must explicitly supply
`range` to select a byte window. The main complete-byte preview reader does not
iterate through those windows. See [reader wiring](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/index.ts#L99-L104)
and [read dispatch](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/index.ts#L258-L280).

PDF.js receives complete `data` bytes. Pages render near the visible viewport,
but that rendering policy is distinct from file fetching: this path does not
use a PDF URL or `PDFDataRangeTransport` for on-demand file ranges. The renderer
copies retained bytes before Worker transfer. See [PDF initialization](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/src/client/pdf/runtime.ts)
and [document presentation](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/ui-sidebar-documentpreview/README.md#L64-L66).

### Binary transport

Connection builds a standard `FormData` response: a JSON `metadata` field carries
the RPC result and attachment paths, while `bytes-N` fields carry Blobs. The
browser parses `response.formData()` and reconstructs `Uint8Array` fields from
`Blob.arrayBuffer()`. Normal results and failures remain JSON.
See [Host serialization](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/connection/src/rpc-host.ts#L376-L392)
and [browser decoding](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/client/connection/src/client/rpc.ts#L61-L134).

This avoids Base64 expansion and its text decoding step. It still materializes
complete payloads and makes copies; it does not establish a process-memory cap
or a custom chunk-by-chunk preview protocol.

### Office conversion

The browser submits Session/path coordinates for conversion without first
downloading the complete Office source. The Host authorizes with a one-byte
workspace-file read, checks metadata, reserves conversion capacity, and reads
the source through the filesystem under the converter's own input budget.
The ordinary 32 MiB complete-file cap therefore does not override Office's
50 MiB input policy. Source versions are checked before and after the read.
The current result contains native PDF bytes. See [renderFile](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/document/office-to-pdf/src/index.ts#L177-L223).

The provider and browser have separate bounded caches and cancellation owners.
The provider's default cache retains up to 8 PDFs/128 MiB; the browser cache is
8 PDFs/64 MiB. These limits do not include all caller buffers, engine memory,
multipart framing or PDF.js rendering. See [Office ownership and budgets](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/document/office-to-pdf/README.md#L49-L67).

## Implications for rustX

The inspected rustX checkout is `3c355f152d405a201482d53b6e45497df4f44a7f`.
Its [Session reader](../src/tools/session_files.rs),
[private native adapter](../web-console/host/file-read.ts),
[browser coordinator](../web-console/src/client/session-files.ts), and
[document conversion](../web-console/host/documents/operation.ts) jointly enforce
the current policy. The cached `origin/main` at `832b9e54` also retains the
512 KiB browser read and text-render constants.

Recommended design direction:

1. Separate text-page, byte-window, complete-file and conversion budgets. Use
   DSH's 5,000-line/2 MiB page and 32 MiB complete-file defaults as reference
   values for validation, not as already-approved rustX requirements.
2. Give text, Markdown and code a paged loading contract with source identity,
   version, next offset and EOF. Define browser retention independently; DSH's
   growing prefix alone is not a total-memory guarantee. Preserve literal text
   at page boundaries and never concatenate different source versions.
3. Carry complete binary files outside the ordinary Base64 JSON response.
   Reuse standard binary HTTP/multipart mechanisms where they fit. Every hop
   matters: changing only Host-to-browser encoding leaves rustX's native-to-Host
   1 MiB message restriction in place. Preserve the ordinary App Server budget
   and design the private file carrier explicitly. Application-level binary
   chunks are an implementation choice, not a prerequisite demonstrated by DSH.
4. Let Office conversion read its authorized source on the Host under its own
   budget; a browser full-source read need not precede conversion. rustX still
   needs its existing original-delivery, digest/snapshot, reauthorization and
   physical-settlement guarantees expressed in that contract.
5. Keep original Download independent of renderer success. DSH's delivered-file
   card mainly offers native open/reveal actions; those are not evidence of a
   rustX-compatible large-file download implementation.

DSH permits file reads outside the Session workspace when its filesystem can
read them, as documented by [the read method](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/api/workspace-files/src/index.ts#L225-L241).
rustX must retain its committed-delivery coordinates, original Conversation/root
mapping, descriptor checks, stale-result fences and cancellation settlement.
Sharing renderer/loading conventions does not require sharing DSH's path
authority, plugin system, caches or broader HTML/image execution policy.

## Validation evidence

A Node 24.19.0 probe executed the exact upstream `cutPage` function after type
stripping, replacing only its error class with a local equivalent. It confirmed:

- A 600 KiB text page succeeds.
- A 4,020,000-byte text file returns successive 5,000-line pages with `eof: false`.
- A single page above 2 MiB fails with `workspace-file/too-large`.

A separate platform probe round-tripped 600 KiB through Node's standard
`FormData`, `Response.formData()` and `Blob.arrayBuffer()` without Base64 and
verified byte equality. This checks the platform mechanism, not the full DSH
Connection implementation. DSH was not launched and its full test suite was not
run; the end-to-end ownership conclusions above are source-based.

No DSH implementation source is copied into rustX by this research note.
