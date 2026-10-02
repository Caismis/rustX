# Issue #431 implementation and validation audit

Validated on Linux x86_64 on 2026-10-02. The protected original checkout stayed
on `main` at `89f35bdf5794b7e30f23ee7ce96a5dabdf262551`, with no tracked changes
and its existing untracked `.playwright-mcp/`. Initial remote inspection found
no overlapping open PR. Work used the independent
`/home/caismis/Documents/codes/rustX-issue-431` worktree and
`issue-431-file-delivery-preview` branch, based on that exact fetched main SHA.
The post-validation fetch found the same main SHA; no integration was needed.

Repository instructions, CI lanes, native Tool/settlement/history owners,
Session catalog/allocation/deletion, Product Host authorization, current generated
clients, safe Markdown, resource lifetimes, and #430/#443 geometry were audited.
The pinned reference audit is in [issue-431-reference-audit.md](issue-431-reference-audit.md).
No new Harness source was copied. Existing attributed rustX presentation code
was reused; source-inventory hashes/imports and image-size's MIT notice were updated.

## Final architecture review

| Required invariant | Actual owner and evidence |
| --- | --- |
| Delivery follows successful ordinary settlement/commit | Native `Present` returns typed facts only after validation and cancellation checks. `AgentLoopExecution::commit_tool_result_batch` uses the existing atomic `append_canonical_batch_with_events`; no special loop branch. A gated post-validation cancellation returns no facts. |
| Typed metadata alone authorizes cards | `AgentTranscript` reads successful canonical transcript Tool messages. It never derives cards from foreground completion, prose, arguments, basenames, or Tool JSON. Failed declarations expose no cards. |
| File identity differs from ArtifactId | `ToolExecutionResult.deliveries: Vec<SessionFileReference>` is distinct from managed `artifacts`. Closed Web sources and distinct `session/fileRead` / `artifact/read` methods preserve both kinds. |
| Browser paths confer no authority | Product Host accepts exact attachment/message/index coordinates. Configured native roots come from the Host; the native read resolves only committed metadata. |
| History cannot silently retarget | Original Conversation plus cwd device/inode and normalized relative path remain unchanged in clone/fork/branch copies. Native catalog lookup resolves the original published node, not the selected default node. Cold reopen, unrelated same-named files, paging and fork reads are covered. |
| Every filesystem read rechecks authorization | Native admission, current original mapping, attachment authority, exact configured root, and post-read fences are checked. Allocation ownership excludes physical source deletion. Product Host checks its scope before and after native work. |
| Containment holds at open/read | `openat`/`O_NOFOLLOW` directory descriptors anchor every ancestor. The root identity and regular leaf inode are checked; retained edges are checked before bytes and before return. Gated ancestor/leaf symlink and directory/target swaps fail without returning replacement bytes. |
| Special files cannot block/leak | No-follow leaf metadata must be regular before nonblocking open; the opened descriptor must still be the same regular inode. Directory, FIFO, symlink and `/dev/null` tests reject. |
| Preview does not run Agent/model work | File reads use existing attachment read authority, canonical store lookup and SessionController allocation access. No Agent Loop mutation or Attempt resume occurs. Native and browser tests assert unchanged model-request counts. |
| Obsolete completions cannot publish | Captured exact target, native authority revision, Product Host observation, disposal, AbortSignal and keyed effect/live flags fence successes, errors and loading. Image callbacks compare URL identity. Gated A/B and authority/attachment/close tests verify URL reclamation. |
| Terminal settlement stays unique | Present uses ordinary foreground-only sequential Tool registration and operation-owned settlement. Repeated declarations are separate invocations; ordinary canonical result ownership remains unchanged. |
| Managed Artifact semantics remain intact | Immutable ArtifactStore bytes, ArtifactId, artifact/read, spill/capacity, 256 KiB and existing transfer/URL limits remain. Existing native Artifact tests and actual managed text downloads pass. |
| Geometry and scroll ownership remain intact | Existing RightPanel/ChatViewport/turn-navigation owners are reused. No new sidebar or scroll effect was added. Full browser acceptance retains reference images; native distant reading and file open/close pass in both locales and narrow/wide layouts. |

The complete normative contract, including deterministic order/duplicates,
mutable bytes, unavailable history and security limits, is in
[file-delivery.md](file-delivery.md). In particular: 1–8 entries, first normalized
duplicate wins including its description, every entry validates, descriptions
are at most 512 Unicode characters, and each successful later call is a separate
declaration. No automatic scanning or write-to-delivery inference exists.

Session reads/download bytes are limited to 524,288 bytes; base64 is at most
699,052 characters under the 1 MiB native frame. Native reads, Host operations,
browser transfers and Session-file URLs are each limited to two. Managed Artifact
policy remains 262,144 bytes, two transfers and sixteen URLs. Markdown input is
bounded by the source byte policy; fatal UTF-8 decoding and static raster limits
(4096 per axis, 4,194,304 pixels) retain original-byte Download on viewer failure.
One authorized byte source provides rendering and a finite download URL. Raw HTML,
active SVG, executable URLs and local/remote Markdown image loading stay inert
under the existing safe renderer.

The string-only `ArtifactResources.readText()` path was removed. Both source kinds
now provide original bytes for Download alongside decoded text, with original
filenames and no rendered-HTML serialization. The optional managed-output
continuation is boxed to keep the expanded result's layout bounded; its serialized
shape and managed-output semantics are unchanged.

## Protocol and generated clients

Rust DTOs generate App Server v31 schema/TypeScript/fixtures together. All Web,
TUI and protocol imports/handshakes/classification use that vocabulary; v30 files
are removed. Runtime Client is strictly v56 for typed delivery metadata; every
older peer is rejected. Current/future-version fixtures use v56/v57 respectively.
Event envelope 1, SQLite 49, Session catalog 13 and subagent IPC 29 remain unchanged.
There are no version aliases, compatibility readers or parallel delivery records.

## Validation commands and results

Toolchains: Rust 1.98.1, Node 24.21.0 and pnpm 11.13.1. Python 3.12 and the existing
frozen provider-emulator environment were prepared. Docker was unavailable;
the checked-in browser script's supported `CONTAINER_ENGINE=podman` selected the
same pinned Playwright image:
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.

Native tests used `TMPDIR=/var/tmp/rustx-431-validation.metety`: pre-existing
`/tmp/.git` and project documents caused ordinary workspace discovery to escape
fixture roots. The isolated temporary root fixes the environment without changing
production confinement, assertions or test selection.

| Exact command (from repository root unless specified) | Final result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --all-features --locked` | Pass; real executables built before browser/native process acceptance |
| `TMPDIR=/var/tmp/rustx-431-validation.metety RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | Pass: 4,085 tests over 19 targets; zero failures; eight existing ignored tests |
| `pnpm --dir protocol/app-server generate` | Pass; Rust/Node generation only, no manual generated edits |
| `pnpm --dir protocol/app-server check` | Pass; no schema/client/fixture drift |
| `pnpm --dir protocol/app-server typecheck` | Pass, including closed file identities/request/error contracts |
| `pnpm --dir tui typecheck` | Pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Pass: 895 tests, zero skipped |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | Pass: 72 files, 1,350 tests |
| `pnpm --dir web-console check:i18n` | Pass |
| `pnpm --dir web-console check:provenance` | Pass: 147 source records, 132 production package notices |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Pass: 148 tests; production build and artifact provenance pass; reference images unchanged |
| `pnpm --dir dev typecheck` | Pass |
| `pnpm --dir dev test` | Pass: 38 tests |
| `uv sync --frozen` (in `test-support/fake-provider`) | Pass |
| `uv run --frozen pytest` (same directory) | Pass: 51 tests |
| `git diff --check` | Pass |

Dependencies were installed with `pnpm --dir protocol/app-server install --frozen-lockfile`,
`pnpm --dir tui install --frozen-lockfile`, `pnpm --dir web-console install --frozen-lockfile`,
and `pnpm --dir dev install --frozen-lockfile`. The new exact image-size dependency
and lockfile were prepared together; notice generation was checked afterward.

Additional focused commands run during development:

- `cargo test --all-features --locked --lib session_file`: seven filesystem,
  scope-copy/deletion and controlled mapping-loss tests passed; cold reopen is
  also included in the final full run.
- `cargo test --all-features --locked --lib tools::native::present`: order/repeats
  and gated cancellation passed; the initial malformed-input test incorrectly
  expected structural Err instead of ordinary PreflightOutcome::Rejected. It was
  corrected to assert the actual rejection contract and passes in the full run.
- `cargo test --all-features --locked --lib committed_present_reads_exact_native_scope_through_current_authorized_attachment`:
  the final native protocol regression passed, including real canonical commit,
  historical paging/fork, full attachment-target routing, current bytes, missing
  and oversize, allowed-root denial, capacity, Artifact isolation and zero extra
  model requests. Its fork case caught and fixed per-Conversation attachment-ID
  collisions in the original weak lookup.
- `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e file-delivery.spec.ts`:
  both focused browser tests passed with actual downloads. Initial harness repairs
  addressed the second ordinary Tool approval, remote Download.saveAs, and the
  Chinese canonical-region selector; no assertions were weakened.
- Isolated initialization and Runtime Client subsets diagnosed the temporary-root
  issue and strict version fixtures; all are covered by the passing full run.

Earlier full Rust validation first failed on temporary-root discovery and stale
v55/current-next-version fixtures (3,398 passed / 52 failed in the library target).
After those repairs the library passed, and a contracts guard caught the obsolete
documentation sentence about v29 rejection. Correcting it to v30 made the final
all-target run pass. Earlier compiler/lint runs caught result constructors,
exhaustive error consumers, result layout and two new test lint issues; these were
fixed. TUI negotiation expectations were updated to v31. No legitimate test was
deleted, skipped, quarantined, retried automatically, or given larger timing bounds.

The eight existing ignored tests are two instrumented performance measurements,
one committed document-fixture regeneration test, and five credentialed live
provider smoke tests. All provider-emulator suites were required and executed.
Native macOS validation was not available locally; Linux and pinned Chromium
results do not establish macOS filesystem/process behavior.
