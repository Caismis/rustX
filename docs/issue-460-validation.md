# Issue #460: offline ask_user schema and feedback evidence

The implementation starts from `origin/main` at
`3c4b85a627e87383f244600e8b19887f103b82f0`. It removes generation-facing regex
constraints and improves native argument feedback without changing Agent Loop
retry/terminal behavior, canonical result storage, or provider adapters.
The branch was rebased onto the subsequent upstream commit
`6d3bd27d6902d0ba0508b98aea700c2be754d569`, which adds only GitHub notification
scripts and workflows. A post-rebase comparison confirms that the validated
Rust, schema, test, and UI surfaces are unchanged. Formatting and the newly
added notification tests were checked again after the rebase.

## Ownership and diagnostics

`src/tools/native/ask_user.rs` owns its compiled input definition, descriptions,
Serde deserialization, and the narrower native 2–4 option authoring bound.
`src/events/interaction.rs` remains the shared semantic validator. Its existing
duplicate checks now retain the first index and report both zero-based paths.
Reserved-label errors describe the runtime-owned choice and a correction
without embedding the submitted label. Text, count and preview errors also
identify their affected fields.

Native argument diagnostics are capped at 1,024 bytes, including the prefix,
using the existing UTF-8-safe text bound. Malformed input uses
`serde_path_to_error` and follows only trusted property names in the compiled
schema: raw Serde error messages and unknown authored keys never enter the
diagnostic. This formats a deserialization failure; it does not revalidate
business rules. Shared MCP choice cardinality and `allow_custom`-scoped
reserved labels remain unchanged.

One earlier boundary also needed diagnostic sanitation: reserved `__rustx_*`
invocation metadata is rejected by the registry before native normalization.
`src/tools/schema.rs` now formats that existing error with a fixed root path,
reserved namespace and removal instruction, without echoing the arbitrary key.
The typed rejection and execution flow are unchanged. A dedicated owner test
and native preflight regression cover oversized private keys.

## Compiled schema delta

[`schema-before.json`](../tests/fixtures/ask_user/schema-before.json) was captured
from the actual compiled native definition at the starting revision.
[`schema-after.json`](../tests/fixtures/ask_user/schema-after.json) captures the
changed definition. The regression compares complete compiled structures after
removing only descriptions and the five old patterns; every other schema
property must remain equal.
The captured parameter schemas differ by exactly five removed patterns and
eight updated descriptions, with no other structural changes.

| Field under `questions.items` | Removed | Preserved string bounds |
| --- | --- | --- |
| `question` | `pattern: "\\S"` | 1–4096 |
| `header` | `pattern: "\\S"` | 1–16 |
| `options.items.label` | `pattern: "\\S"` | 1–60 |
| `options.items.description` | `pattern: "\\S"` | 1–1024 |
| `options.items.preview` | `pattern: "\\S"` | 1–8192 |

The root and nested objects still reject unknown properties. The schema keeps
1–4 questions, 2–4 options, all required fields, optional non-null preview,
and `multi_select: false` as the default. No replacement regex or `uniqueItems`
claim is introduced. Descriptions explain unique question text and option
labels, meaningful text, the automatic custom-answer choice, and preview mode.
The wire tests compare the compiled schema with the schema each adapter sends.

## Deterministic regressions

| Location | Test / evidence |
| --- | --- |
| `src/tools/native/ask_user.rs` | `compiled_schema_delta_only_removes_five_patterns_and_updates_guidance`: recursive pattern and structural snapshot checks |
| Same | Existing `every_model_facing_questionnaire_bound_is_preflighted` plus new whitespace, malformed-input, reserved-label and byte-bound cases: authoritative validation survives schema removal |
| Same | `whitespace_only_authored_fields_remain_invalid_without_schema_patterns`; `malformed_argument_diagnostics_keep_safe_paths_without_authored_values`; `missing_fields_and_null_preview_have_safe_actionable_diagnostics` |
| Same | `reserved_label_diagnostics_name_the_field_and_correction_without_the_label`; `argument_diagnostic_bound_counts_bytes_without_splitting_utf8`; `reserved_runtime_metadata_rejection_is_bounded_without_echoing_the_private_key` |
| `src/tools/schema.rs` | `reserved_invocation_diagnostic_never_echoes_an_authored_key`: the earlier metadata boundary is safe for every execution policy |
| `src/events/interaction.rs` | `questionnaire_duplicates_report_both_nonadjacent_paths_without_values`: exact deterministic duplicate paths and correction text |
| Same | `shared_choice_cardinality_remains_wider_than_native_authoring`: shared choices still accept 1, 5 and the shared maximum number of options |
| `tests/scripted/tools/ask_user_feedback.rs` | `unicode_arguments_survive_stream_normalization_history_and_questionnaire_publication`: exact Chinese question/header/labels/descriptions/preview through scripted fragments, production JSON acceptance, normalization, canonical history, publication and answer |
| Same | `rejected_ask_user_feedback_reaches_next_request_before_scripted_correction`: rejected call never enters the executor or interaction coordinator; next request sees the exact canonical failure; corrected proposal executes normally |
| `tests/provider/ask_user_feedback.rs` | `chat_serializes_actionable_ask_user_rejection_and_compiled_schema`, `responses_serializes_actionable_ask_user_rejection_and_compiled_schema`, `anthropic_serializes_actionable_ask_user_rejection_and_compiled_schema`: actual loopback request bodies preserve schema, feedback, identity and mixed-batch order |
| Same | `chat_assembles_exact_unicode_ask_user_arguments`, `responses_assembles_exact_unicode_ask_user_arguments`, `anthropic_assembles_exact_unicode_ask_user_arguments`: real adapters assemble one-character SSE fragments into the exact original Unicode JSON, then native normalization preserves all strings |

The Agent fixture uses watch-channel gates, not sleeps. The second request is
inspected before the scripted model can emit its correction. The invalid call
has one failed canonical result with empty tool-owned content, no executor
start/completion facts, no pending questionnaire, and no interaction-request
fact. Its projection is exactly `Tool call failed: <preflight diagnostic>`.
After releasing the gate, a distinct corrected call publishes one questionnaire,
receives a deterministic option answer, and commits one successful result.
A final text-only turn verifies both ordered results. No model retry is
scheduled; the two tool proposals are explicit FakeModel turns.

The provider tests serialize a three-call success/failure/success batch through
each real adapter to a loopback fixture server. Chat uses `role: tool` with
`tool_call_id`; Responses uses `function_call_output` with `call_id`; Anthropic
uses `tool_result` with `tool_use_id`. Exact projected text and
original call ordering are asserted, with no dropped or duplicate results.
The unchanged shared projection enforces its 64 KiB result budget. String
versus text-part-array protocol encoding remains unchanged.

## Validation

All commands run in the independent worktree with Rust 1.98.1, Node 24.19.0,
pnpm 11.13.1 and Python 3.12.11. Rust compilation uses
`CARGO_TARGET_DIR=/home/capi/Documents/codes/rustX/target` and
`CARGO_BUILD_JOBS=2`; tests use `RUST_TEST_THREADS=2` initially and four
threads for the corrected-environment full run, with
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Live-provider tests remain ignored.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --offline --workspace --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --offline --bins --all-features` | Pass |
| `TMPDIR=<fresh /var/tmp directory> RUST_TEST_THREADS=4 cargo test --offline --workspace --all-features --all-targets --no-fail-fast` | Completed, exit 101: 4,131 passed, three failed, eight ignored across all targets. Library: 3,479 passed, three failed, three ignored. All nine integration targets pass (632 tests, five ignored); examples pass (20 tests). Exact exceptions are documented below. |
| `cargo test --offline --workspace --all-features --doc` | Pass: nine documentation tests |
| `cargo run --offline --all-features --example check_test_lanes` | Pass: Linux CI test-lane coverage holds |
| `pnpm --dir protocol/app-server check` | Pass: generated schema and TypeScript match checked-in files |
| `python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v` | Pass after rebase: all 15 notification tests from the new upstream commit, with no real network requests |
| `cargo test --offline --all-features --test provider ask_user_feedback` | Pass: all six new protocol/Unicode regressions |
| Final compiled library harness: `tools::native::ask_user::tests`, `events::interaction::tests`, `reserved_invocation_diagnostic_never_echoes_an_authored_key` | Pass: 15 native, 20 shared Questionnaire and one reserved-metadata test |
| `cd test-support/fake-provider && uv sync --frozen && uv run --frozen pytest` | Pass: 54 tests |
| `pnpm --dir tui typecheck` | Pass |
| `pnpm --dir tui test` | Pass: 943 tests, one skipped (104 suites), including questionnaire and real-child transport coverage |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir dev typecheck` | Pass |
| `CHOKIDAR_USEPOLLING=1 pnpm --dir dev test` | Pass: 41 tests |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console check:i18n` | Pass |
| `pnpm --dir web-console check:provenance` | Pass: 276 source records, 264 production-package notices |
| `pnpm --dir web-console build` | Pass; existing large-chunk warnings |
| `pnpm --dir web-console test` | Initial run: 2,504 passed, 16 failed, four skipped; see environment details below |
| `cd web-console && pnpm exec vitest run --maxWorkers=2 --exclude test/document-integration.test.ts --exclude test/office-resource-boundary.test.ts` | Pass: 2,507 tests, four skipped (148 files passed, one skipped), including questionnaire and PTY suites |
| `cd web-console && CHOKIDAR_USEPOLLING=1 RUSTX_E2E_PREVIEW_PORT=18553 RUSTX_E2E_FIXTURE_PORT=18554 bash scripts/browser-tests.sh --grep questionnaire` | Pass on rerun: all five pinned-browser cases, including the native snapshot and English/Chinese light/dark compaction-questionnaire continuations |

The initial Web run started before the worktree had a `target` link to the
shared build directory; eight PTY failures disappeared after that prerequisite
was corrected. The remaining eight office-conversion/resource-boundary failures
come from host inotify exhaustion: `fs.watch('/tmp')` and the required
`fs.watch(cgroup.events)` fail with `EMFILE`, while the process file-descriptor
limit is 1,048,576. The user inotify instance limit is 128. Systemd user-service
admission, configured cgroup limits, `cgroup.kill` access and Bubblewrap
namespace admission passed. `node web-console/scripts/check-office-sandbox.ts`
reproduces the watcher failure on both this worktree and unchanged starting
sources; those two suites are explicitly excluded only in the successful
supported-environment rerun, not claimed as passing.

The unmodified development launcher's Web-ready test also timed out because
Vite's watcher failed with the same `EMFILE`. It reproduces on the starting
sources and passes with Vite's supported `CHOKIDAR_USEPOLLING=1` setting. No
host limits, system policy or product code was changed to bypass these checks.

The first browser run passed three cases and timed out in the two English
compaction cases under heavy concurrent host load. Trace inspection placed
both failures before questionnaire execution: an empty-context compaction
response and the next scripted model response missed existing five-second
assertions. With lower host load, all five cases passed unchanged in 41.4
seconds. The tests, deadlines and reference images were not modified.

The initial full Rust run exposed a separate temporary-directory problem:
this host has `/tmp/.git` and `/tmp/rustx.toml`. Tests that deliberately omit
their own workspace marker discovered `/tmp` as the workspace, triggering
unrelated launch-path validation failures. The same failing test binary passes
when `TMPDIR` points to a fresh directory under `/var/tmp`. The full rerun uses
that isolated temporary root and `--no-fail-fast` so every target executes.
The superseded run was interrupted after the replacement run had started;
it is not claimed as a completed full-suite result.
The existing `/tmp` files were not removed or edited as an environment repair.
All 32 initialization, launch and settings failures from that first run pass
in the corrected full run. The five process-configuration failures from the
separate initial integration run also pass under the isolated root.

The unchanged Rust regression
`local_runtime::session::tests::reservation_tests::r01_reservation_inspects_zero_existing_session_directories`
also requires a native inotify instance. Its isolated rerun under the clean
temporary root fails at `src/local_runtime/session/tests/cfg3_reservation.rs:145`
while constructing that observation probe, with `EMFILE`, before exercising
the reservation. This is the same host resource limit as the Office failures.

The other two full-library failures hit the unchanged 30-second liveness
guard at `tests/scripted/app_server/mod.rs:359`, rather than a contract
assertion. Both pass on the same binary when run individually:

```sh
rustx_test_tmp=$(mktemp -d /var/tmp/rustx-460-response-cut.XXXXXX)
TMPDIR="$rustx_test_tmp" target/debug/deps/rustx-ad7a488926e9b215 --exact local_runtime::session_runtime_manager::tests::protocol::completed_response_cut_is_shared_by_branch_and_fork_and_distinct_from_retry --nocapture
rmdir "$rustx_test_tmp"

rustx_test_tmp=$(mktemp -d /var/tmp/rustx-460-final-slot.XXXXXX)
TMPDIR="$rustx_test_tmp" target/debug/deps/rustx-ad7a488926e9b215 --exact local_runtime::session_runtime_manager::tests::protocol::concurrent_final_slot_is_reserved_before_composition --nocapture
rmdir "$rustx_test_tmp"
```

These reruns pass in 14.40 and 2.01 seconds respectively. The initial run's
`scripted_suites::agent::tool_scheduling::mixed_foreground_background_group_does_not_wait_for_detached_terminal`
failure also passes both individually and in the corrected full run. No
timeouts, fixtures or production behavior were changed to make these pass.
The evidence supports host-load sensitivity; the full run is still reported
with its three failures, not as wholly green.

## Testing limits

No real inference API was accessed, and no live provider credentials were
used. New tests use scripted FakeModel events, static fixtures and
loopback-only HTTP servers. These checks establish rustX schema, argument
integrity, canonical feedback and protocol translation. They do not establish
actual Kimi K3 constrained-decoder behavior or another real provider's
generation behavior. Native macOS boundary validation requires macOS CI.
