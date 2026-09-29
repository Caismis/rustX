# PR #425: Image input across all supported protocols

## Repository state and scope

Implementation worktree: `/home/caismis/Documents/codes/rustX-issue-412`.
Branch: `issue-412-image-reading-bash-description`.
Starting PR head: `7a4f283331968663b24f0c857623d4dffff3ea47`.
Fetched main and merge base: `5d2e382154a9d3434afb860a6892d76e98ad6bfb`.
Starting topology: five commits ahead, zero behind main; clean worktree.
Neither remote had moved. No integration merge was necessary. Starting PR CI
36507851274 and main CI 36493246236 had completed successfully.

No App Server vocabulary changed: v29 remains the sole generated lineage.
No model declaration is automatically expanded. Static PNG bounds, canonical
artifact ownership and Bash description semantics remain unchanged.

## Capability and transport

Image capability covers both User and ToolResult image input. The admitted
invocation intersects explicit model declarations with adapter and runtime
support. All three adapters advertise Image, and each uses the same frozen
invocation and request-owned resolved bytes.

| Protocol | User mapping | ToolResult mapping | Correlation |
| --- | --- | --- | --- |
| Anthropic Messages | `image/source` base64 block | image block inside `tool_result` | `tool_use_id` |
| OpenAI Responses | `input_image` data URL | SDK `FunctionCallOutput::Content` with ordered text/image parts | `call_id` |
| Chat Completions | SDK `image_url` data URL | typed multimodal `role: tool` content array | `tool_call_id` |

Chat intentionally follows the extended OpenAI-compatible/vLLM multimodal Tool
message contract. The pinned async-openai 0.41.3 Tool enum is text-only, so the
adapter defines one typed BYOT Tool extension using SDK content parts. There is
no provider/model/hostname check, alternate Chat mode, placement capability,
synthetic User fallback, capability downgrade or image-stripping retry. Remote
incompatibility surfaces as an ordinary provider/protocol error.

The Tool Plane now owns the bounded ordered text/image projection previously
implemented inside Anthropic. All three adapters use it. Each adapter owns wire
encoding; the OpenAI adapter module shares only data-URL encoding, not artifact
resolution. Canonical history contains no base64 or SDK values.

Responses fresh-history replay omits empty Stored continuation markers, which
carry only a previous-response pointer and no reasoning input. This permits
image/text model switching and full canonical reconstruction. Actual reasoning
text without lossless provider-native state still fails explicitly.

## Prior review findings: already fixed at the starting head

- **Lineage:** `local_runtime/session/artifacts.rs` materializes and remaps the
  exact prepared lineage cut into the destination ArtifactStore before
  publication. `managed_images_materialize_exact_clone_fork_branch_cuts` proves
  Clone/Fork/Branch ownership, source mutation/deletion independence, source
  Session deletion independence, pre-image cuts and preparation/publication
  cleanup. No source-store dependency or second image store exists.
- **Request bound:** `model/images.rs::project` retains the newest 16 image
  occurrences and projects older references to text, including repeated artifact
  occurrences. Canonical history remains unchanged. Parallel Tool execution
  cannot bypass this request-owned decision.
- **Context:** `context/images.rs::ImageEstimator` adds
  `1024 + ceil(width * height / 256)` per selected image occurrence, measured
  from validated immutable PNG bytes. Text projection charges text. Dimension,
  SQLite replay, fit/compaction and observed-prefix identity tests remain.
- **Web Settings:** `settings/forms/controls.tsx` includes `read_image` in
  `nativeTools`, outside `policyTools`. Root and named-Agent authoring tests
  cover selection, authoritative Save/reread/Reload, removal and policy exclusion.
- **Git settlement:** `runtime/workspace.rs::supervised_workspace_git` owns the
  metadata guard inside the task owning physical process settlement. The real
  FIFO-gated caller-abort regression proves same-repository exclusion and
  independent-repository progress. No production Git change was needed here.

The canonical image-deletion fixture still derives residue from
`ProductRoot::existing(...).root()`. Bash cancellation still uses deterministic
native settlement evidence; the shell trap marker is not a grace-period
requirement. No confinement, cancellation grace, retry or timeout was changed.

## New deterministic evidence

- `catalog_requires_effective_image_and_selected_implementation`: image and
  text-only declarations across all three adapters; configured intent remains
  necessary.
- `all_protocols_transport_user_and_ordered_tool_images`: exact PNG bytes,
  User images, interleaved Text/Image/Text/Image Tool results, two correlated
  calls, no synthetic User turn, image/text/image switching, unchanged canonical
  messages, and missing/corrupt/over-bound rejection before HTTP.
- `chat_parallel_images_switch_and_reconstruct` and
  `responses_parallel_images_switch_and_reconstruct`: real native `read_image`,
  17 parallel successful distinct snapshots, exactly 16 transported images,
  explicit Tool/function-output call IDs, frozen Attempt adoption gate,
  text/image switching after source replacement/deletion, canonical retention,
  and exact full outbound request equality after SQLite reopen/reconstruction.
- `seventeen_parallel_image_reads_leave_a_valid_provider_continuation`: the same
  native request-bound and exact reconstruction proof for Anthropic.
- Existing Anthropic round-trip coverage retains text-only Workflow child and
  summary requests. Generic retry/overflow paths continue through
  `stage_model_turn` / `finalize_model_turn`, which use the same frozen projection
  and resolution. Adapter changes introduce no alternate retry constructor.
- Archive export still collects canonical managed references and reads them
  through the owning Conversation's ArtifactStore. Deletion remains Session-owned.

## Validation

Local platform is Linux. macOS and credentialed external endpoints are not claimed.
No CI configuration, timeouts, retry counts, ignored tests or screenshot baselines
were changed in this transport expansion.

### Full Rust validation

All final commands passed:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --bins
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features
git diff --check
```

Full Rust: **4,058 passed, 0 failed, 8 existing ignored**. Breakdown:
library 3,433 (+3 ignored), CFG3 catalog 26, managed-output 5, conformance 26,
contracts 28, durable 129, process 63, provider 168 (+5 ignored), subagent 45,
Tools 135. Binaries/examples contain zero tests and completed successfully.

### Focused commands

Each command below passed its one exact selected test (zero failures):

```sh
cargo test --all-features local_runtime::session::tests::deletion_tests::lifecycle::session_deletion_reclaims_committed_image_without_a_published_reference -- --exact --nocapture
cargo test --lib --all-features runtime::workspace::tests::concurrent_children_have_distinct_deterministic_paths_and_refs -- --exact --nocapture
cargo test --lib --all-features runtime::workspace::tests::caller_abort_keeps_metadata_gate_until_physical_git_settlement -- --exact --nocapture
cargo test --lib --all-features local_runtime::session::tests::managed_images_materialize_exact_clone_fork_branch_cuts -- --exact --nocapture
cargo test --all-features --test tools bash::background_cancel_records_term_before_trap_and_physical_terminal -- --exact --nocapture
cargo test --all-features --test tools bash::bash_background_cancellation_uses_the_same_process_group_path -- --exact --nocapture
cargo test --all-features --test tools bash::bash_kill_escalates_when_term_is_ignored -- --exact --nocapture
```

Additional focused checks:

- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test provider capability_boundary::all_protocols_transport_user_and_ordered_tool_images -- --exact`: **1 passed**.
- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test conformance parallel_image -- --nocapture`: final **3 passed**, including exact reconstruction on every protocol.
- `cargo check --all-targets --all-features`: passed after correcting the SDK enum path.

### Client, emulator and generated contracts

| Directory | Commands | Result |
| --- | --- | --- |
| `test-support/fake-provider` | `uv sync --frozen`; `uv run --frozen pytest` | 51 passed |
| `protocol/app-server` | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm generate`; `pnpm check`; `pnpm typecheck` | Passed; v29/fixtures regenerate without drift |
| `tui` | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 894 passed |
| `web-console` | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm check:i18n`; `pnpm check:provenance`; `pnpm test`; `pnpm build` | 1,156 tests passed across 64 files; 145 source records verified |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e` | 143 passed, 8.5 minutes |
| `dev` | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm test` | 37 passed |

The full E2E command includes live image decoding/lightbox, reconnect/replay,
Fork/Branch image continuity, read_image Settings Save/Reload/removal, named
configuration surfaces, current-main Trajectory, and Bash description/command
disclosure. Web has no Clone control; the native exact lineage test covers Clone.

### Development failures and corrections

These attempts are not represented as passing:

- Initial compilation used `InputItem::FunctionCallOutput`; the pinned SDK owns
  that discriminator in `Item`. A test request index also needed an explicit
  checked `u64` → `usize` conversion, and the reconstruction resolver needed its
  existing read closure rather than a store reference.
- Clippy initially rejected two documentation backticks and six implicit default
  test arguments. Both were corrected without lint exemptions.
- The initial emulator fixture lacked required Chat reasoning replay configuration
  (0 passed / 3 failed); the next scenario incorrectly excluded historical
  `read_image` call names from text requests (1 passed / 2 failed). Tool-catalog
  admission is distinct from immutable historical call identity.
- Responses switching exposed the empty Stored continuation marker bug
  (2 passed / 1 failed). The adapter repair above resolves it without accepting
  unrepresentable reasoning text.
- An initial full Rust run stopped after the library target: 3,431 passed,
  2 failed, 3 ignored. Both failures were obsolete Chat Image capability-view
  expectations, now updated to the requested contract.
- A superseded overlapping run still carried those old assertions, reported four
  additional Python/Workflow failures, and stalled in a Workflow test. It was
  terminated and has no successful final total. The affected cases passed in the
  subsequent current-code run; no cause is asserted solely from that later pass.
- Another full run passed the library (3,433), CFG3 targets (26 + 5), then stopped
  at conformance (24 passed / 2 failed): new assertions incorrectly assumed one
  User message in every continuation. They now respect Chat runtime-context
  messages and the stored Responses tail, while explicitly rejecting movement of
  Tool images into User content. The corrected focused matrix passed 3/3.

### Exact current CI Linux groupings

All four commands from `.github/workflows/ci.yml` also passed:

| Command | Passed / failed / ignored |
| --- | --- |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,239 / 0 / 3 |
| `cargo test --test contracts --test provider --all-features` | 196 / 0 / 5 |
| `cargo test --lib --all-features -- boundary_suites::` | 194 / 0 / 0 |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 429 / 0 / 0 |

Protocol generation/check/typecheck was repeated after the final production edits,
with no generated drift. TUI typecheck and all 894 tests also passed again against
the rebuilt native binaries. The final feature diff preserves current-main
Trace/Trajectory ownership, exactly one v29 lineage, canonical storage confinement,
the unchanged Bash TERM grace, and the existing physical Git settlement owner.
