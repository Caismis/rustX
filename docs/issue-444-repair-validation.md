# PR #444 authority and child-capability repair

The repair began at `5d24301a5999838afdba5835bcd66aec053597cf` on the existing
`issue-431-file-delivery-preview` branch in
`/home/caismis/Documents/codes/rustX-issue-431`. Fetched main was
`89f35bdf5794b7e30f23ee7ce96a5dabdf262551`. The protected original checkout was
not modified. The reviewed green CI covered implementation behavior but did not
establish Product Host trust: any ordinary authenticated client could supply
`allowed_roots`, and a second ordinary connection was incorrectly treated as a
trusted Host in the regression.

## Authority and ownership

The public `session/fileRead` Method is removed. App Server v32 is the sole
mandatory protocol; generated v31 files are removed. Runtime Client v56 and the
typed canonical delivery/reference formats remain unchanged. Result/error DTOs
remain shared types for the Host carrier; they do not expose an ordinary read
Method or grant permission.

The launcher provisions a separate 32-random-byte Product Host credential for
the addressed native process and trusted Node Host. The native process receives
an owner-only regular credential file with `--product-host-token-file`; Node
receives its value in private configuration. Neither the browser bootstrap nor
catalog receives that credential. Startup rejects shared transport credentials,
symlinks, non-regular secret files, wrong owners and group/other permissions.
The private `/product-host/file-read` WebSocket handshake requires this credential,
distinct from the browser token. Its selected subprotocol never echoes the secret.
Client names have no trust meaning. The one-operation private carrier uses the
existing strict, lossless attachment-identity codec, not native numeric JSON.

The browser supplies only attachment/message/index coordinates and its observed
Host scope. Current registrations authorize file bytes; configured possible
picker locations and display classification alone do not. The trusted Host sends
canonical current roots only after scope/endpoint admission. No new association
cache, delivery database, browser path authority, or general permission framework
is introduced. Original historical Conversation/native mapping and root device/
inode checks remain mandatory.

Registration-root changes linearize at metadata commit and synchronously abort
owned Host reads. The Host rechecks its authority, abort state and root availability
before browser publication. Host close/replacement and caller disconnect abort
too. Native socket EOF/close, process shutdown and native credential replacement/
removal cancel the captured native authority. Native checks occur before lookup,
at descriptor-open/read fences, after the read and before transport publication.
Previously completed reads are not retroactively erased. The exact contract is
in [file-delivery.md](file-delivery.md).

`present` remains root-selectable and uses ordinary Tool settlement. It is absent
from child native registration and the model inventory. Explicit child selectors
fail through existing ScopeUnsupported/ChildUnsafeSelector admission. Forced
frozen composition fails before a child runtime exists; there is no substitution,
promotion or child compatibility mode.

## Caller audit and deterministic evidence

| Caller/category | Correct owner after repair |
| --- | --- |
| Browser resources | Product Host HTTP carrier; coordinates only, never native roots/credential |
| LocalWorkspaceHost | Current registration policy and two owned, abortable private native sockets |
| Native protocol scenario | Actual authenticated private WebSocket with a different credential; no second ordinary connection pretending to be Host |
| Ordinary protocol rejection | Removed method with all valid coordinates and exact guessed cwd returns -32601; browser token fails private admission |
| Filesystem internals | Existing descriptor reader tests, unchanged |
| Generated contracts | Public Method union cannot express session/fileRead; shared result/error types confer no authority |

The boundary regression commits a real Present result, reads exact original
bytes through the private lane, and denies ordinary bypass. It retains historical
paging/fork scope, unrelated same-named files, current mutable bytes, deletion,
root-policy denial, oversize, capacity, Artifact isolation and zero extra model
requests. Three deterministic gates stop after leaf descriptor open, before any
bytes: native credential removal, socket close and attachment revocation. Each
asserts the owned read fails, no successful bytes are published, and capacity is
returned. The socket-close case waits for the native cancellation token, not a
timer. Existing controlled mapping/root replacement and filesystem race tests
remain intact.

The real boundary entry point is
`boundary_suites::app_server_file_read::committed_present_reads_exact_native_scope_through_current_authorized_attachment`.
It reuses the established native fixture scenario and is selected by both Linux
and macOS boundary lanes; it is no longer hidden behind macOS's manager-contract
skip. Root/child capability tests cover selection, named profile/override admission,
model inventory and forced composition.

Node policy tests explicitly use a gated carrier fake to test registration
removal, Host close, caller abort and obsolete callbacks. They are not the proof
of native trust; real native sockets and browser acceptance provide that evidence.
Browser acceptance retains actual original-byte Unicode/spaces downloads for
Markdown/text/code/raster/unsupported formats, mutable reopen, English/Chinese,
narrow/wide geometry, scroll stability and unchanged model counts. It adds an
ordinary direct bypass and real Host registration revocation.

## Preserved invariants

`src/tools/session_files.rs` is unchanged: retained directory descriptors,
O_DIRECTORY/O_NOFOLLOW, no-follow leaf metadata, fd identity and regular-kind
checks, nonblocking leaf open, bounded reads and pre/post edge verification.
Traversal, symlink/ancestor/leaf replacement, FIFO/directory/device, root identity,
mapping and attachment tests remain. Typed delivery facts still enter history
only through successful ordinary canonical ToolResult commit. Managed ArtifactId,
artifact/read, immutable bytes and 256 KiB limits remain separate. Session files
retain mutable identity, 512 KiB limits and finite ownership. Preview resources,
safe Markdown, original-byte downloads, stale-response fences, object URL cleanup
and #430/#443 geometry/scroll owners are unchanged.

The pinned DeepSeek Harness reference audit remains
[issue-431-reference-audit.md](issue-431-reference-audit.md). This repair copies no
new external source. Source-inventory hashes/imports follow regenerated protocol
imports; dependency notices are unchanged.

## Validation

Validation commands and results against the repair tree:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --all-features --locked` | Pass |
| `TMPDIR=/var/tmp/rustx-431-validation.metety RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | Pass: 4,089 tests across 19 targets, eight existing ignored opt-in tests |
| `pnpm --dir protocol/app-server generate` | Pass: Rust/schema/client/fixtures generated together |
| `pnpm --dir protocol/app-server check` | Pass: no generated drift |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir tui typecheck` | Pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Pass: 895 tests |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | Pass: 73 files, 1,355 tests |
| `pnpm --dir web-console check:i18n` | Pass |
| `pnpm --dir web-console check:provenance` | Pass: 147 source records, 132 production notices |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Final pass: 148 tests; earlier failures and controlled comparisons below |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e file-delivery.spec.ts` | Pass: two real-browser cases |
| `pnpm --dir dev typecheck` / `pnpm --dir dev test` | Pass: 38 tests |
| `uv sync --frozen` / `uv run --frozen pytest` in `test-support/fake-provider` | Pass: 51 tests |
| `cargo run --all-features --locked --example check_test_lanes -- --job rust-boundaries` | Pass: Linux test-lane coverage |
| `git diff --check` | Pass |

Focused native commands passed:

- `cargo test --lib --all-features --locked committed_present_reads_exact_native_scope_through_current_authorized_attachment`:
  real private admission, byte-identical success, ordinary bypass rejection,
  credential/socket/attachment revocation gates and preserved history/security.
- `cargo test --lib --all-features --locked private_host_read_requires_exact_wire_identity`:
  exact lossless wire identity is mandatory; native numeric identity is rejected.
- `cargo test --lib --all-features --locked present`: 23 tests.
- `cargo test --lib --all-features --locked session_file`: seven tests.
- `cargo test --test subagent --all-features --locked present`: two tests.
- `pnpm --dir web-console test -- test/product-host-file-read.test.ts`:
  five deterministic Host-policy/abort tests.

During repair, browser acceptance exposed a private-carrier identity encoding
mismatch masked by native numeric test fixtures. The private carrier now uses the
existing strict lossless codec; native fixtures and the real Node/browser path
exercise the same encoding. Earlier compilation/type/lint failures and stale
version expectations were corrected before the final passing runs. Protocol
check initially reported intentional uncommitted generation changes; the final
check against the committed repair finds no drift.

The first full browser run passed 147/148, with one unchanged narrow dark-theme
composer reference differing by 16 rounded-border pixels (maximum channel delta
18, identical dimensions). Composer source, references, comparison policy and
timing bounds were not changed. Three independent isolated runs passed:
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e agent.spec.ts --grep 'composer primary seat, uploads and context stack dark 390' --repeat-each=3`.
The initial artifacts are preserved in `/tmp/rustx-444-composer-evidence` for local
inspection. The second full run passed 147/148, with a different Settings focus
failure at `settings-presentation.spec.ts:580` after reopening the dialog. Twenty
isolated runs on untouched reviewed head `5d24301a` in a temporary reference
worktree and twenty on the repaired head both passed. Both used the same pinned
dependencies/container and this command:
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e settings-presentation.spec.ts --grep 'confirming a removal settles focus' --repeat-each=20`.
No causal regression was established by that comparison; it does not prove the
intermittent failure cannot recur. Its artifacts are retained locally in
`/tmp/rustx-444-settings-evidence`. No Settings code, focus assertion or timeout
was changed. The final full run, with no concurrent compilation/generation/file
edits, passed **148/148** in 9.1 minutes. These diagnostic runs are explicit
validation records, not automatic retries, changed baselines or quarantines.

The post-validation fetch still found main at
`89f35bdf5794b7e30f23ee7ce96a5dabdf262551`; it did not move, so no integration
was needed. The existing PR branch is used; no replacement PR or merge is made.

Linux validation uses the existing isolated
`TMPDIR=/var/tmp/rustx-431-validation.metety`, required frozen provider emulator,
Rust 1.98.1, Node 24.21.0 and pnpm 11.13.1. Browser acceptance uses the checked-in
digest-pinned Playwright container through its supported Podman option; a Browser
plugin is not available. No tests are weakened, skipped, quarantined or given
longer timing bounds.

Native macOS execution is unavailable locally. The repair's boundary entry point
is included in macOS test selection; the earlier reviewed-head macOS green result
does not validate this repaired head. Listing the built test binary with the
workflow's two skip prefixes retains the private-read, wire and root/child tests.
Attempting `cargo run --all-features --locked --example check_test_lanes -- --job rust-platform-boundaries`
on Linux correctly refuses: that discovery must run natively on macOS. This is a
platform limitation, not a passing macOS result.

Subsequent hosted run `37010526549` tested repair head `2835d219` through merge
`4cd2754`: macOS boundaries, both Linux lanes, lint, protocol and TUI passed.
Full Web conformance failed on the narrow dark composer rasterization variant.
The independent investigation, bounded evidence and repair validation are
recorded in [issue-444-web-conformance.md](issue-444-web-conformance.md).
