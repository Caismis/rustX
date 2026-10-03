# Issue 434 implementation decision

Audit base: `260994fc27ebc1ef1f767b6e6aa21a01c5f676c6`. App Server v33;
Runtime Client v57. No open PRs at initial audit. Primary checkout untouched.

Use authenticated control preparation with ordered names and lengths and a client
operation identity established before dispatch. Mint a random 256-bit single-use
capability on a separate `/session-upload/` WebSocket route. Remote clients resolve
against their selected native origin; owned stdio clients receive a loopback port.
This follows archive preparation and transport ownership. Dedicated binary framing
uses the existing tungstenite/browser/Node implementations, avoiding a new HTTP
request-body parser and browser streaming-request portability restrictions.
Control WebSocket binary messages remain forbidden. No Product Host involvement.

Policy: 2 MiB/file, 4 MiB/batch, 8 files, 2 prepared-or-active transfers, 64 KiB
binary messages. Acknowledgement of each chunk bounds client sending. Native input
remains a bounded vector: at most 8 MiB payload across the two transfers plus bounded
framing buffers. Full buffering before native materialization rejects incomplete
streams before file creation and avoids a new streaming storage abstraction.
The transfer permit must remain owned through native settlement. Native materialization
runs off the async worker. Maximum admitted in-flight file bytes is 8 MiB; committed
uploads remain Session-owned and are not a total storage quota.

A native allocation records its operation identity before filesystem side effects.
Read repair distinguishes absent, unresolved, failed-before-ready and ready. A ready
read returns the original receipts. Transport preparations/active transfers prevent
an absent native allocation from being mistaken for permission to retry while a
carrier can still begin mutation. Unknown durable outcomes remain unresolved.

Intake uses stable local identities, bounded per-file outcomes inside the count
bound, and atomic bounded summary rejection above it. Send requires every retained
item to be ready (or an accepted first-submission draft). Retry requires known
pre-commit failure; uncertain operations offer a read only. View retirement never
rolls back native storage.

## DSH reference audit

Pinned independent checkout: `deepseek-ai/deepseek-harness` at
`639ed015397290b3745d163aafe02ffee4aa3f84`.
Inspected `packages/client/ui-conversation/src/client/skeleton/InputBar.tsx`,
`input/editor/keymap.ts`, `service.ts`, and `packages/client/file-upload/src/`
`protocol.ts`, `index.ts`, `http-route.ts`, `client/runtime.ts`.
Actual chain: InputBar -> conversation service retained attachment map/workers ->
fileUpload client worker XHR/fetch -> authenticated connection fetch route ->
FileUploads.uploadStream -> attachments.saveFileStream -> Host-staged receipt map.
Adopt shared intake, stable attachment identity, retained presentation ownership,
remove-without-storage-rollback, and send gating. Reject Host-owned storage/staged
receipts, Remote base64 fallback, automatic queue/rebinding, generic error retry,
Cordis and Lexical. No DSH source copied for the new carrier/native correlation.

Reference file SHA-256 evidence (audit only; no source copied):

- `packages/client/ui-conversation/src/client/skeleton/InputBar.tsx`: `2716aa3582251a5cfbc77268f95c39ebfcf44a4d3c43cd65d0726ddb7a9c7649`
- `packages/client/ui-conversation/src/client/input/editor/keymap.ts`: `0c992afc82935e8445a1120cf4f1cab9614c7550180c18be582f106e2c1bbf45`
- `packages/client/ui-conversation/src/client/service.ts`: `7e4d2488407a6c125145aa12ce733762622f6fb6089ed9b20507447d510a2845`
- `packages/client/file-upload/src/protocol.ts`: `cf535d3e55e49465e07806b71b0a40df30709f4e9af8d28ce464a44ce6f17978`
- `packages/client/file-upload/src/index.ts`: `6ef4dcc203b96399f34b9d735ef9a6641424ea84f3883c90f71715f905dac0b2`
- `packages/client/file-upload/src/http-route.ts`: `dc91debb0862f7c153559dfc172cc95f185f47c25337feb27ec4f4c5e5db76e0`
- `packages/client/file-upload/src/client/runtime.ts`: `857d776f153ca3e8694f9ee98e72e37fd7bdf39e0c47a9db82a78e22b25203d9`

The carrier is a dedicated WebSocket stream, not an HTTP upload body. Content-Length does not delimit uploads. Ordered native-admitted lengths, finite binary message limits and the terminal finish exchange enforce actual bytes, including truncation and trailing data. Redirects are forbidden by the WebSocket opening contract; descriptors cannot select another origin or path.
