# Workflow file and authoring foundation validation

Issue: [#510](https://github.com/Caismis/rustX/issues/510). Base:
`9c794f7638d4c6070ad453fd7269b9f7c563d070` (the fetched `origin/main`).
Implementation branch: `feat/workflow-file-analysis-510`, in an independent
worktree. The final commit and exact-head FULL verification are recorded in the
implementation PR. This establishes the native foundation; #511 remains the
complete editor product-delivery gate. No automatic or self merge is requested.

## Owners and contracts inspected

The source audit covered `runtime/workflow` definitions, expressions, compilation,
inspection, execution, Tool registration and Review; local runtime discovery,
authoring candidate/check/capacity and configuration/source mutation; App Server
protocol/schema/dispatch; the generated protocol and its actual Web/TUI consumers;
Web Workflow document/machines/ports/Canvas/Inspector/view store; authoring,
programs, architecture and invariants documentation; repository instructions,
test architecture and CI profile selection. Open scheduler/durable PRs were
inspected but are not dependencies of this change.

The final ownership split is:

- `workflow_file` owns the complete YAML document, canonical serialization,
  typed bounded editor metadata and presentation repair diagnostics. The
  compiler receives `document.definition` only.
- File revision identifies complete observed source bytes; canonical candidate
  identity identifies complete normalized bytes. CAS, receipts, copy/delete and
  lost-response recognition retain presentation edits. Program digest and Tool
  schemas contain only executable contracts. Admitted programs remain frozen.
- Schema `description` is an executable annotation, bounded to 4096 UTF-8 bytes
  per annotation and existing aggregate schema limits. Native structural proofs
  ignore annotation differences; Tool/provider contracts preserve them.
- `runtime/workflow/authoring` reuses compiler flow, entry-scope intersection,
  reference, schema and Model-output proofs. It reports declarations separately
  from availability and compatibility and never constructs a program. Unselected
  authored identities lower through native identity constructors at compilation.
- `workflow/analyze` is inert and exact-target. App Server protocol 60 and all
  actual generated consumers advance together. No dependency preparation or
  credential access is added.
- Web translates authored consumer positions and renders native facts. Request
  ownership fences successful and failed completions by port/authority, target,
  immutable document, edit generation, consumer/query and request sequence.
  The wire echo also checks target, resource, generation, query and candidate
  revision spelling. Native candidate revision and browser generation remain
  separate identities.

See [the authoring contract](workflow-authoring.md) for the complete metadata,
scope, diagnostics and bounds tables. The file bound is 1 MiB, expanded document
768 KiB, editor data 256 KiB and executable definition 512 KiB. Metadata has at
most 1024 aggregate entries/128 notes, Block-address depth 8, finite coordinates within
±1,000,000 and dimensions 16–8192. Analysis queries are at most 16 KiB with search
256 bytes, reference/expression depth 32 and 1–256 choices; traversal visits at
most 2048 reference paths. Analysis data is at most 256 KiB and the correlated
response at most 512 KiB. Presentation diagnostics have a 64-entry truncation
sentinel. Invalid geometry/capacity is rejected; orphaned addresses are preserved
with separate presentation warnings.

The obsolete localStorage coordinate authority, separate view-history path and
check-gated output catalog have been removed. Only viewport preferences stay
local. Rename/delete/subtree-copy and layout changes update one complete document
transaction, preserving field drafts/history. Notes in surviving Blocks remain.

## Deterministic test-to-invariant mapping

| Invariant | Owner evidence |
| --- | --- |
| Nested metadata, Markdown, order, exact numeric values and optional metadata | `workflow_file::tests::complete_file_round_trip_preserves_nested_metadata_markdown_order_and_numbers`; `missing_metadata_is_optional_and_unknown_or_duplicate_fields_are_rejected` |
| File revision versus executable identity, Tool schema and frozen program | `workflow_file::tests::presentation_changes_source_bytes_but_not_program_digest_graph_or_model_contracts`; existing admitted execution tests |
| Descriptions are preserved, bounded and non-asserting | `workflow_file::tests::annotations_are_bounded_and_do_not_change_structural_compatibility`; provider structured-output adapter tests with root/leaf descriptions |
| Finite metadata, stale references and unknown fields have distinct outcomes | `workflow_file::tests::metadata_bounds_and_stale_references_have_distinct_non_destructive_outcomes`; local `presentation_damage_is_diagnosed_without_erasing_content_or_claiming_type_failure` |
| Complete-file CAS, copy/delete and lost acknowledgements in User/Workspace scopes | local `complete_file_revisions_fence_metadata_writes_copies_deletes_and_lost_ack_repair`; existing receipts/recovery/number/source-authoring suites |
| Incomplete instruction/graph/selection remains analyzable; Save/Check remains authoritative | native authoring `declarations_and_lexical_proofs_do_not_require_a_valid_program`, `actual_editor_unselected_identities_are_analyzable_but_never_compiled`; local `unselected_authored_identities_round_trip_for_analysis_but_cannot_replace_a_file` |
| Whole objects/arrays, required versus optional paths, text output and joins | native authoring `required_paths_whole_objects_arrays_and_type_compatibility_use_native_proofs`, `joins_intersect_predecessor_values_and_never_export_branch_business_data` |
| Duplicate local IDs, projected branch args, private children, Loop parent/result scopes | native authoring `parallel_projections_and_duplicate_local_ids_keep_their_exact_block_identity`, `loop_initial_body_result_and_parent_scopes_are_distinct` |
| Unknown Tool contracts stay unknown; a real node named result keeps identity | native authoring `external_tool_expected_types_stay_unknown_and_bad_output_contracts_stay_pending`, `a_real_node_named_result_retains_its_producer_identity` |
| Unknown producer schemas preserve scope facts: self/forward/bypassed references unavailable, guaranteed upstream references pending | native authoring `unknown_output_contracts_preserve_producer_scope_facts` tests selected and catalog entries with incomplete and repaired schemas; real App Server/browser Workflow editor regression verifies disabled and selectable choices |
| Output inspector distinguishes loading, failure, truncation, incomplete declaration and ready output | `workflow-model.test.tsx` uses controlled analysis responses/rejections and a new candidate generation; its Picker test disables three unknown-schema scope violations and preserves pending upstream selection |
| Bounded/truncated analysis and nullable incomplete schema on the wire | native `analysis_bounds_search_and_truncation_are_explicit`; generated complete/incomplete request-response fixtures and TypeScript type contract |
| Analysis has zero model/Tool/child/preparation/credential effects | local `analysis_is_inert_correlated_to_complete_candidate_and_separate_from_save_validity` and unselected-identity test use the existing 11-counter static-effects probe |
| Out-of-order successes/failures, target/authority/generation/field/query fences | `web-console/test/workflow-analysis.test.ts`, using explicitly controlled deferred promises; independent review also exercised the actual React hook close/reopen race |
| Atomic structured metadata rename/delete/copy and persistent layout history | Web analysis/document/machine/editor-identity suites, including derived Parallel output orders and fresh note identities |
| Real native read/edit/save/copy, incomplete picker and keyboard position | `workflow-editor.spec.ts` native Product Host/App Server test, preserving Markdown and model-visible descriptions and asserting zero provider requests |

## Validation results

The review corrections add the producer-presence matrix, controlled Inspector
states and native browser regression mapped above. Corrected-head owner checks,
independent re-review and final FULL verification results are recorded in the PR.
The initial implementation's local verification evidence is retained below:

| Command/check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo check --all-targets --all-features` | Passed |
| `cargo build --bins --all-features --example generate_app_server_protocol --example generate_schemas --example document_artifact_fixture` | Passed |
| Native schema/protocol generators, `node protocol/app-server/generate.mjs`, then repeat generation and compare SHA-256 | Passed; no generated artifact drift |
| `cargo test --lib --all-features workflow` | 252 passed, zero failures |
| `pnpm -C protocol/app-server typecheck` | Passed, including nullable incomplete schema fixtures and query correlation helper |
| `pnpm -C web-console typecheck`, `pnpm -C tui typecheck`, `pnpm -C dev typecheck` | Passed |
| `pnpm -C web-console check:i18n` | Passed |
| `pnpm -C web-console check:provenance` | 322 source records and 309 package notices verified |
| `pnpm -C web-console build` | Passed, including production provenance check |
| Web Workflow owner suites and independent review | Initial 131 Workflow tests passed; independent corrected snapshot passed 46 focused tests and the controlled actual-hook stale-failure race; subsequent final small fixes passed 17 focused tests |
| Corrected Workflow browser flows in the pinned container | All six scenarios passed: five existing flows together, then the new metadata/picker flow after its save-settlement wait was corrected; strict existing screenshot references were unchanged, Model Axe check passed, new scenario asserted zero provider requests |
| `pnpm -C web-console test --maxWorkers=1` | Completed: 3563 passed, 26 failed, four skipped. Eighteen Office environment failures and eight five-second UI timeouts under concurrent build/browser load; eight unhandled Office rejections. No assertion or timeout was weakened |
| `pnpm -C tui test` with required emulator | 1303 passed, one failed, one skipped. The one WebSocket child-transcript gate timeout passed unchanged in isolation |
| `.github/scripts` Python unittest discovery | 64 passed |
| `benchmarks/runtime` Python unittest discovery | 37 passed |
| `pnpm -C dev test` | Orphan-process settlement could not complete under this container's PID 1; stopped the waiting runner after recording the timeout |
| `git diff --check`, staged whitespace check | Passed |

Complete native and browser invocations were also started locally. Their final
outcomes, any isolated timeout retries, the final commit SHA and the exact-head
FULL CI result are recorded in the implementation PR. The `full-verification`
label selects every lane, including required Office resource-controller and
macOS coverage. The final HEAD must have a successful **Merge verification**
verdict; local targeted results alone do not establish that verdict.

Browser QA used the production preview at `http://127.0.0.1:5173` (the focused
run used port 5185), viewport 1440×1000 and existing responsive/narrow scenarios.
The editor's Model configuration/Text/JSON Schema screenshots and the existing
strict Workflow library/editor/narrow references were inspected or compared.
No rasterizer allowance, baseline or browser image was changed. The added picker
was exercised against the real native current-candidate API, then the test
completed instruction editing, keyboard movement, save confirmation and copy.

## Environment and delivery limits

Local execution uses Linux, Rust 1.98.1, Node 24.19.0 and the repository-pinned
pnpm 11.13.1. The CI Node pin is 24.22.0. The connected Browser runtime is not
exposed in this session; browser checks therefore use the repository's existing
Playwright fallback with its immutable Linux image and strict screenshot
comparisons. No host browser/fonts replace that screenshot authority.

The local container has no functional `systemd --user` resource-controller
session and PID 1 does not reap orphaned test children. Office sandbox/converter
and orphan-process settlement failures must be reported as environment limits,
not converted to skips or weaker assertions. FULL CI keeps these lanes enabled,
as well as the macOS lanes unavailable locally. No live provider credentials or
model requests are needed for this issue. Full Dify-style UI, collaborative
editing, arbitrary expressions and runtime redesign remain out of scope.
