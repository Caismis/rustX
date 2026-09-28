# Issue #412 implementation record

## Checkout isolation

- Primary checkout: `/home/caismis/Documents/codes/rustX`, branch `main`.
- Primary HEAD, initial fetched base and latest integrated `origin/main`:
  `f4c044e9f9aa519254b3f7279de847a4df0ae992`.
- Origin: `https://github.com/Caismis/rustX`.
- Before creating the implementation checkout, branch/HEAD/status, remotes,
  existing branches and the complete worktree inventory were inspected.
- Primary status before and after implementation: `?? .playwright-mcp/`.
  Its tracked files, branch and pre-existing untracked directory were preserved.
- New worktree: `/home/caismis/Documents/codes/rustX-issue-412`.
- New branch: `issue-412-image-reading-bash-description`, created directly
  from fetched `origin/main`. Later fetches found no main movement.
- All development, dependency preparation, generated files, builds and tests
  were performed from the implementation worktree. The implementation commit
  is recorded by the PR; no development branch is used as its base.

## Ownership and request path

| Step | Existing owner and change |
| --- | --- |
| Model/config resolution | `model/invocation.rs`: declared ∩ adapter ∩ runtime; only Image input expands |
| Admission | Conversation coordinator captures primary invocation, resource generation and image publication fact under its existing lock |
| Tool publication/dispatch | `AgentExecution` derives one immutable filtered `ToolRegistry` from its frozen invocation and capability lease |
| Defensive execution | Native `read_image` receives that same invocation; missing/text-only authority fails before file access |
| Filesystem validation | Native tool uses ordinary cwd/absolute-path semantics and bounded static-PNG decoding |
| Durable image result | Conversation `ArtifactStore` reserves/writes the validated snapshot; canonical `ImageReference` carries only its ID |
| Request materialization | Provider-neutral ephemeral byte map is resolved through the runtime artifact boundary and excluded from serialization |
| Provider transport | Anthropic adapter encodes PNG into actual User/image and ToolResult/image blocks; call/result identity and order remain canonical |
| Historical requests | Frozen text-only projection replaces images with artifact-reference text; canonical Ledger and artifacts remain unchanged |
| Summary/children | Summary input remains a text transcript with artifact IDs; child Attempts apply their own frozen invocation gate |
| Clients | Native capability/Attempt facts, canonical Tool arguments/results, background Bash metadata, and bounded Trace source/arguments feed both clients |

Session-model mutation remains subject to the repository's existing Busy
adoption gate. Tests hold real provider gates in both directions, prove a running
Attempt cannot be replaced, then prove the next eligible admission uses the new
model/catalog. Existing deterministic configuration/resource/catalog admission
and lease-race suites run with the new registry derivation.

Inventory and configured intent remain inspectable separately from the selected
model's gated catalog. The admitted `read_image_active` fact comes from native
admission, never a client calculation. A later configuration does not rewrite
historical requests or the active Attempt.

## Supported image contract

| Adapter/protocol | Effective Image input | Placement | Format |
| --- | --- | --- | --- |
| Anthropic Messages | When declared | User and ToolResult image blocks | Static PNG |
| OpenAI Chat Completions | No | Text projection only | None |
| OpenAI Responses | No | Text projection only | None |

The encoded limit is 256 KiB, dimensions 4096 per side, total pixels 4,194,304,
output decode allocation 16 MiB, and the decoder's internal budget 32 MiB.
Requests resolve at most 16 distinct images. PNG extension/signature/content,
CRC/end, dimensions, decode allocation and animation checks precede success.
APNG, JPEG, GIF, WebP, File input and Assistant image output are unsupported.
Missing, invalid or unsupported image transport fails before HTTP. No format
conversion, OCR, network fetch, provider upload or second attachment store exists.

Cancellation is checked before reading, after physical read/decode settlement,
and after storage. Failed/cancelled operations publish no successful reference.
Reservations stay within existing conversation retention and deletion ownership.
The source can be changed or removed after success without changing its result.

The real emulator scenario reads a native PNG, checks the subsequent base64 wire
payload, deletes the source, switches to text, then switches back to image input.
It also reopens SQLite and reconstructs every retained request: text projection
keeps call/result correlation, image-capable history keeps the original opaque
reference, and durable evidence contains no base64.

## Bash and presentation

`description` is optional, nonblank and 1–160 Unicode scalar values. It is
presentation intent only. Command, timeout, environment, execution mode,
approval policy, cancellation and process settlement keep their existing owners.
Unknown justification/escalation fields remain rejected. Tests compare actual
native outputs and registry authority with and without descriptions, and follow
background metadata through start, settlement and serialization.

TUI collapses to sanitized descriptive text and exposes the command in detail;
image reads have their own renderer and an opaque-reference fallback. Its
current Tool surface has no raster preview transport. Web uses its existing
managed-artifact gallery and React text rendering, with command disclosure for
both foreground and background Bash. Trace retains bounded canonical arguments,
real shell source, result artifacts and stable call identity. Generated App
Server schema/TypeScript and an independent native incremental capture were
regenerated. There are no handwritten replacement protocol types.

## Reference comparison

DeepSeek Harness stayed clean and read-only at
`477b4f420553e8a52c2fbccc464d7561b239c443`. Inspected paths include:

- `packages/fs/tool-fs/src/read-image.ts` and `src/index.ts`.
- `packages/attachment/attachment-local/src/request-image.ts`.
- `packages/llm/llm-pi-ai/src/adapter.ts` and `src/context.ts`.
- `packages/shell/tool-bash/src/index.ts`.
- `packages/terminal/tool-terminal/src/render.ts`.
- `packages/client/ui-tool/src/client/tool/components/ToolRow.tsx`.

Adopted: explicit route capability checks, genuine image results, descriptive
intent separate from commands, disclosure, actual execution status, and
server-owned image loading. Deliberate differences: Attempt-frozen authority,
existing managed artifacts instead of DSH attachments, static PNG only, no
normalization/variants, optional bounded Bash description, and one foreground/
background semantic projection. DSH's required description and separate
escalation justification are not copied into rustX authority.

## Validation

Validation uses Linux, Rust stable, Node 24.21.0, pnpm 11.13.1, Python 3.12.13
and the repository's digest-pinned Playwright Chromium container via Podman.
The current CI workflow was read, including real binary prerequisites,
mandatory emulator selection, launcher checks, localization, provenance,
schema drift and browser acceptance.

| Command (from its owning directory) | Final outcome |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed; actual rustx and supervisors built |
| `uv sync --frozen` in `test-support/fake-provider` | Passed |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | 4,040 passed, zero failed, eight existing intentionally ignored tests |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features image` | 18 selected tests passed |
| `corepack install` and `pnpm install --frozen-lockfile` in both clients and protocol | Passed |
| TUI `pnpm typecheck` | Passed |
| TUI `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 894 passed, zero skipped |
| Web `pnpm typecheck` | Passed |
| Web `pnpm check:i18n` | Passed |
| Web `pnpm check:provenance` | Passed: 143 source records, 131 production notices |
| Web `pnpm test` | 1,129 passed |
| Web `pnpm build` | Passed; existing large-chunk advisory remains |
| Web `CONTAINER_ENGINE=podman pnpm test:e2e` | 127 passed, one Settings screenshot timeout (details below) |
| Web `CONTAINER_ENGINE=podman pnpm test:e2e test/e2e/settings-presentation.spec.ts --grep 'desktop: six pages'` | Unchanged failed case passed in isolation (9.5 seconds) |
| Web targeted activity/chat/Trajectory browser checks | Image decode, Bash command expansion, reconnect and native Trajectory passed |
| Protocol `pnpm generate`, `pnpm check`, `pnpm typecheck` | Passed; schema and generated TypeScript agree |
| `cargo run --example generate_schemas` and `git diff --exit-code -- schemas` | Passed, no configuration-schema drift |
| `RUSTX_PROJECTION_CAPTURE=web-console/test/fixtures/incremental-native.json cargo test --lib --all-features incremental_projection_independent_snapshot_capture -- --nocapture` | Passed; independent native capture regenerated and reviewed |
| `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test` in `dev` | Passed; 37 launcher tests |
| `git diff --check` and `git diff --cached --check` | Passed |

The full browser command is **not reported as entirely green**. Its one final
failure was `settings-presentation.spec.ts:84`, a 120-second timeout waiting for
the Settings dialog during screenshot capture after accessibility inspection.
The same unmodified case passed alone. Neither test timeouts, assertions,
screenshot baselines nor retry settings were changed. This is not labelled a
pre-existing defect: it was not reproduced on baseline. Feature-specific
browser tests passed, and desktop/mobile screenshots were visually inspected.

Earlier development checks exposed and resolved missing constructor fields,
a noncanonical test identity, a test counting existing SQLite/output files as
artifacts, an obsolete text-only image-failure expectation, a stale binary/schema
pair and an ambiguous image locator. Earlier managed-Python preparation failures
and one Web unit timeout did not recur in the final full Rust/Web runs; no
baseline-failure claim is made. The default `pnpm test:e2e` initially could not
find Docker; the repository-supported `CONTAINER_ENGINE=podman` path used the
same pinned browser image. No test was disabled and no emulator lane was skipped.

## Self-review and limits

The full diff was reviewed against the recorded latest main for mutable model
lookups, raw capability gates, duplicate storage, provider data in durable
contracts, historical image leakage, unsupported advertised image paths,
description-based execution/approval, missing background/replay projections,
generated drift and unrelated changes. No native web search/fetch is added;
those remain MCP-owned.

The App Server capability projection now requires `configured_tools`; clients
must be regenerated together. Runtime/model request structs also gain explicit
fields. No compatibility shim or migration is added. Existing canonical Bash
arguments remain sufficient for historical command-based presentation.

macOS and credentialed live-provider calls were not run. Deterministic provider
emulation proves transport without credentials or subjective model behavior.
The intentionally narrow protocol/format and TUI preview limits above are part
of the implemented contract, not claims of broader support.

No known functional acceptance gap remains within the documented support matrix.
The final full-browser timeout/isolated-pass discrepancy remains a validation
caveat for review. The PR is intended for review only: no merge, auto-merge or
post-creation CI monitoring is part of this task.
