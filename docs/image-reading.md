# Native image reading and Bash presentation

`read_image` reads a filesystem snapshot into a genuine managed Tool artifact.
It performs no OCR, model inference, conversion, network access, or upload.
Session uploads remain mutable Session-owned files; reading one with this Tool
creates an independent immutable output under the conversation's `ArtifactStore`.

## Capability and admission

Effective capabilities are the intersection of the model declaration, protocol
adapter capabilities, and implemented runtime capabilities. Only Image input
changes; File and image output remain unsupported. The configured native Tool
intent must include `read_image`. Its policy is foreground, parallel, approval
never, like ordinary reading. The admitted registry is derived once from the
Attempt's frozen primary invocation and selected capability generation.
Execution receives that same invocation and refuses missing/text-only authority
before opening a path. Configuration changes cannot mutate this registry.

Selected model configuration and `attempt.model.primary` are separate native
projections. `capabilities.configured_tools` describes configured intent;
`capabilities.tools` applies the selected model's effective gate. Historical
request Tool definitions in Trace describe the exact admitted catalog. The
`attempt.execution_settings.read_image_active` fact is captured by the native
admission owner alongside the frozen model and resource revision.

## Image contract

Input is `{ "path": "screenshot.png" }`. Relative paths use the execution cwd;
absolute paths and symlinks retain native file-tool host semantics. The opened
handle must be a regular file. Nonblocking open prevents FIFO reads from hanging.
Only `.png` (case insensitive) or extensionless PNG files are accepted. Both the
signature and complete decoding are checked; an extension cannot grant support.

- Encoded snapshot: at most 256 KiB, enforced with a bounded read of limit + 1.
- Dimensions: at most 4096 pixels per side and 4,194,304 pixels total.
- Output decode allocation: at most 16 MiB, checked before allocation.
- Decoder internal allocation budget: 32 MiB using `png::Limits` (the decoder
  documents this as best effort, excluding caller-owned output buffers).
- Animated PNG is rejected. JPEG, WebP, GIF and every other format are rejected.
- A model request retains at most 16 distinct managed images.

The decoder validates CRCs and completes the PNG stream, including the end
marker. Validation and artifact creation consume the same encoded byte vector.
There is no second read of the source. Cancellation waits for the blocking read
and decode to settle, then discards its output before artifact creation.
Artifacts follow the existing monotonic reservation and Session deletion owner;
a failed allocation never publishes a successful ToolResult. Cancellation
observed after storage also publishes no successful reference; its reserved
artifact stays under the same conversation retention owner. A successful result
is independent of subsequent modification or deletion of the original path.

## Provider support

| Protocol | Effective Image input | Placements | Formats |
| --- | --- | --- | --- |
| Anthropic Messages | Yes, when declared | User image blocks and image blocks inside Tool results | Static PNG only |
| OpenAI Chat Completions | No | Text only | None |
| OpenAI Responses | No | Text only | None |

Assistant image output, canonical File references, unresolved images, invalid
PNG data and unsupported protocol placements fail before network I/O. Direct
adapter calls remain fail-closed for text-only effective invocations.

The runtime resolves opaque artifact identities through a finite read boundary.
`ModelRequest.images` is ephemeral provider-neutral encoded data and is excluded
from serialization. Canonical ToolCall, ToolResult, Ledger, Request Snapshot and
Trace retain references, never base64 or provider image objects. Anthropic owns
base64 encoding and `image/source` wire construction, including `tool_result`
placement and `tool_use_id` correlation. No canonical User turn is fabricated.

When switching to text-only input, request projection replaces images with a
short artifact-reference sentence. It preserves message identities, Tool calls,
Tool-result correlation and all other content. The durable Ledger is unchanged.
Returning to an image-capable invocation resolves the original managed artifact.
Request reconstruction repeats this policy from the frozen invocation. Summary
requests already use the bounded text transcript with image artifact identities;
they do not accidentally inherit primary image payloads. Child Attempts apply
the gate to their own frozen primary model.

Wire references: [Anthropic Tool result types](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/tool_result_block_param.py),
[base64 image source](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/base64_image_source_param.py),
and [png 0.18.1 decoder](https://docs.rs/png/0.18.1/png/struct.Decoder.html).
The Anthropic adapter uses rustX's existing reqwest/JSON wire implementation;
external SDK types do not cross into the kernel.

## Bash description

Bash accepts optional `description`: 1–160 Unicode scalar values, with
whitespace-only values rejected. Omission is intentional: a command alone is a
complete execution request. The generated schema and executor enforce bounds.
It is model-authored intent, not evidence that a command succeeded or did what
its description claimed. Canonical arguments retain it; typed Job projections carry description and
command together. Trace retains bounded canonical arguments and its existing
authoritative shell-source projection, without duplicating unbounded commands.

The description is never passed to the process runner, used to choose Tool
identity or execution options, or treated as an authorization decision. Existing
command, timeout, environment, cwd, approval and settlement owners are unchanged.
There is no `justification`, `sandbox_permissions`, or escalation contract.
Any future approval justification must be a separate authority-bearing contract,
never an interpretation of this descriptive text.

TUI and Web cards may show description collapsed and always retain command in
expanded details. Missing descriptions use command-based presentation. TUI
presentation replaces control characters; Web renders text through React without
interpreting markup. Actual state and exit codes come only from execution facts.
The image Tool has a dedicated presentation; Web uses existing server-authorized
managed-artifact galleries, while the TUI shows the opaque reference (its current
Tool rendering surface has no raster preview transport).

## Reference comparison

DeepSeek Harness was inspected read-only at
`477b4f420553e8a52c2fbccc464d7561b239c443`:

- `packages/fs/tool-fs/src/read-image.ts` and `src/index.ts`: conditional service
  registration, exact routed modality refusal before read, image output blocks.
- `packages/attachment/attachment-local/src/request-image.ts`: request image
  materialization and variants. rustX deliberately adds no variant/conversion or
  attachment store and uses its existing managed ArtifactId owner.
- `packages/llm/llm-pi-ai/src/adapter.ts` and `src/context.ts`: actual route gate
  and image-bearing context translation. rustX freezes authority at admission
  instead of re-resolving a mutable route during execution.
- `packages/shell/tool-bash/src/index.ts`: description separate from escalation
  justification and command. rustX makes description optional and bounded.
- `packages/terminal/tool-terminal/src/render.ts`: bounded terminal output and
  actual running/exited status stay separate from descriptive intent.
- `packages/client/ui-tool/src/client/tool/components/ToolRow.tsx`: disclosure,
  terminal detail and session-authorized image loader separation. rustX reuses
  its existing Harness-derived disclosure and managed-artifact components.

Current-information retrieval, including web search and fetch, remains owned by
MCP Tool Sources. No native web search or fetch is added.
