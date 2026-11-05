# Issue #527 validation and ownership

Initial base: `origin/main@7e9010035fcb457ac415aa23fb946aba51bd1588`.
Integration base after upstream advanced: `d4645468282dc6df56015fd72d0ad442946694c5`.
Upstream already allocated App Server v70; this change uses v71 and retains
upstream Runtime Client v82. Profile ordering and source-intrinsic Settings
repair contracts are preserved and independently reviewed.
Implementation branch: `codex/issue-527-resource-limits`, in a separate worktree.
The PR head identifies the final implementation SHA; this document is committed
with that implementation, so it does not embed a self-referential commit hash.

## Representation and publication

Skill discovery streams every accepted file and retains only the `SKILL.md`
frontmatter, validating body UTF-8 incrementally. A file inspection uses one
64 KiB buffer and two matching full SHA-256 passes, with actual-length, opened
regular-file, metadata and reopened-name verification. Package identities frame
sorted raw relative paths, lengths and verified file digests. No aggregate or
individual-file byte admission ceiling remains. The child copies into private
staging, verifies the complete copied identity against its frozen binding, checks
cancellation and publishes by rename. A failed cleanup is reported with its
location; the existing child runtime lifecycle retains cleanup ownership.

Managed Python capture owns one anonymous file containing frozen raw package
bytes plus a path/offset/length/permission index. Clones share that ownership;
closing the last descriptor reclaims capture storage. Incremental UTF-8 and
quote/comment scanning records effective declaration spans in that capture;
only those spans are read back and parsed. Huge comments and blank lines do not
become retained raw strings. Capture read errors retain their Storage category.
The existing Python store owns coalesced physical builds,
cancellation and unique private staging. It copies captured bytes off the async
executor, verifies source content, runs dependency setup, verifies complete
prepared state again, checks cancellation and publishes with one directory
rename. Waiters await terminal physical settlement and check their cancellation
before receiving a prepared result. Failed staging removal is reported and
retained by the store for retry before another preparation.

Workflow serialized source and normalized compact JSON are independently bounded
at **16,777,216 bytes inclusive**. Canonical save must satisfy both. The App Server
moves documents using connection- and target-bound anonymous scratch references,
64 KiB raw chunks, verified length/SHA/completion and atomic source CAS. Its
ordinary 1 MiB records and child IPC stay unchanged. App Server v72 removes the
inline document API; all protocol consumers use the same exact-u64 codec and
native authority. The encoded carrier allowance is 32 MiB to accommodate wire
integer strings, while native normalized-document admission remains 16 MiB.

The shared client helper admits an entire read/upload/use/release operation
per native client, before encoding, across independently wrapped request ports.
This prevents five or more concurrent condition analyses from exhausting four
scratch slots. Native request ownership survives transport EOF through the
existing detached-operation path, including hashing/parsing settlement; host
drain cannot report completion while a scratch worker still runs.

The complete retained-limit inventory, allocation assumptions and recovery paths
are in [resource-limits.md](../resource-limits.md). No general size setting,
legacy protocol fallback, cache framework or second configuration authority was
introduced.

## Invariant-to-test mapping

| Invariant | Evidence |
| --- | --- |
| Uncapped Skill files and metadata-only discovery | `skills::identity::large_tests::issue527_large_assets_and_markdown_stream_through_discovery_and_materialization`: 17/64/256 MiB assets, 17 MiB Markdown body, matching copied identity, same-size successor and 55-byte retained header. |
| Real child uses verified large Skill bytes without prompt injection | `tests/subagent/process_conformance.rs`: named child selects a Skill with 17 MiB asset and 17 MiB Markdown body; actual provider request excludes the body marker. |
| Large Skill body remains progressively readable | Native Read published-Skill test uses a 2 MiB body and continuation beyond the old 1 MiB boundary. |
| Python memory does not retain supporting-file bodies | `tools::python::tests::issue527_file_backed_packages_scale_without_retaining_asset_bytes`: 17/64/256 MiB binary assets, anonymous `nlink=0` capture, exact staged digest, live rewrite cannot change captured bytes, reload changes identity. |
| Real Python execution and reload | `mcp_managed::one_folder_serves_multiple_tools_through_one_server_identity`: actual managed FastMCP invocation hashes a 17 MiB binary asset; frozen and successor captures return their own hashes. |
| Complete large source revisions | `issue527_same_size_edits_beyond_old_file_and_directory_byte_budgets_change_revision`: valid 65 MiB Skill/Python content and exact-16-MiB Workflow source change the complete accepted-domain fence on same-size edits after raw observation overflow; incomplete observations never certify equality. |
| Bounded unknown trees | `issue527_unknown_huge_files_do_not_consume_unbounded_observation_io`: a 1 TiB sparse unrelated file causes explicit incomplete observations and **zero** streamed file bytes; complete accepted-domain evidence remains stable. Raw observation retains a 64 MiB logical work budget (two verification reads), independently of package admission. |
| Incomplete unused trees | `issue527_incomplete_unused_tree_keeps_startup_and_settings_usable_with_complete_domain_fences`: startup/settings work, incomplete observations remain unequal, accepted same-size mutations and added invalid candidates change the independent complete domain fence. |
| Mutation/confinement/cancellation and I/O failure | `package_io::tests`: controlled growth, truncation, replacement and same-size overwrite; symlink, FIFO and unsafe components refused; cancellation exits after one 64 KiB callback; injected write error and Linux `/dev/full` ENOSPC propagate. |
| Malformed and declaration-only raw input | A newline-free 256 MiB invalid Skill candidate stops after one 64 KiB read. `tools::python::requirements::tests` streams 192 MiB of comments/whitespace with no retained declarations, preserves nonzero capture offsets and split Unicode/quoted comments, and classifies injected capture-read failure as Storage. |
| No cancelled preparation becomes Ready | `issue527_cancelled_preparation_settles_and_cannot_publish_ready`, existing coalesced-build/owner-exit tests and capability preparation cancellation tests: physical settlement precedes terminal result; no prepared or staging directory remains. |
| Independent Workflow source/expanded bounds | `workflow_resources` and `workflow_file` issue527 tests: exact 16 MiB and +1, old 768 KiB/1 MiB crossings, Unicode, JSON escaping, canonical save/read-back and YAML expansion. |
| Scratch integrity and settlement | `app_server::workflow_documents::tests`: chunk order/size, four-document capacity, expiration, connection/scope/digest binding, corrupt/incomplete data, write failure and close while a parser owns work. |
| Real protocol read/check/save/CAS | `tui/test/workflow-documents-integration.test.ts`: >3 MiB expanded document, source padded to exact 16 MiB and +1, exact-u64 strings, stale CAS, incomplete scratch cannot publish, largest control record below 1 MiB. Rust scripted protocol fixtures exercise the same carrier. |
| Whole-operation admission and physical ownership | The real TUI carrier runs eight concurrent checks plus a read through two wrappers sharing one client, then verifies queue recovery after a failed callback. Browser Branch coverage loads six reference conditions with overlapping edits in both scopes. A barrier-controlled real Stdio EOF test keeps the native request count and host drain pending until the scratch worker exits. |
| Browser editing and execution | `workflow-editor.spec.ts`: >3 MiB Code document load/edit/save through Global direct RPC and Workspace Host; `workflow.spec.ts`: real execution from an exact-16-MiB source, with fixture provider only. |
| Unrelated model selection avoids asset scans | `issue527_model_choice_pages_do_not_rescan_unrelated_large_package_assets`: User and Workspace queries over a 256 MiB asset report zero package-I/O bytes. |
| Retained guards/shadowing | Existing Skill/Python traversal and precedence suites; Workflow graph/editor/runtime suites; MCP resource size/projection suites; native Read, artifact/image/preview, transport and upload suites. Full suite results below distinguish executed from skipped platform cases. |

## Cost measurements

Measurements use the already-built libtest executable with `/usr/bin/time -v`,
not Cargo/compiler peak memory. Generated assets are sparse on input but capture
and installation write the complete logical content. Tests use local fixtures
and the fake uv runner for isolated package cost; real managed execution is a
separate integration test. No paid model calls are used.

On this Linux x86_64 development host (debug binaries, warm filesystem cache,
other compilation/checks running), the isolated package fixture processes gave:

| Fixture | 17 MiB asset | 64 MiB asset | 256 MiB asset | Process peak RSS |
| --- | --- | --- | --- | --- |
| Skill discovery, explicit inspect, copy/verify and successor | 6.67 s | 11.69 s | 43.77 s | 52,900 KiB |
| Python capture, prepare, verify and successor | 3.74 s | 13.03 s | 45.90 s | 63,404 KiB |

The Skill 17 MiB case additionally contains a 17 MiB Markdown body; other cases
have a 55-byte frontmatter header. Python anonymous capture occupied exactly
17,825,849 / 67,108,921 / 268,435,513 bytes, with one installed source copy of
the corresponding size. The last captured snapshot closes its anonymous file.
The concurrent nine-test run (including the separate bounded Workflow graphs)
completed in 71.40 s with peak RSS 259,136 KiB; its Skill 17/64/256 cases took
5.64/11.23/52.16 s and Python took 3.78/11.48/55.78 s. These are measured costs,
not latency promises. Scalar/graph allocations explain why whole-suite RSS must
not be presented as package byte-buffer size. The package buffer stays 65,536
bytes and the retained Skill header stays 55 bytes at every tested asset size.

Cancellation at the first chunk callback consumed exactly 65,536 bytes before
settlement; deterministic preparation cancellation left no staging/prepared
package or in-flight build entry. Unrelated model-choice queries over a 256 MiB
asset consumed **zero** package-I/O bytes.
Each file inspection reads twice; Skill discovery plus explicit inspection,
copy/verification and successor check in the fixture read 10 times the total
package bytes and write one installed copy. Python's fixture captures twice,
verifies staging before and after dependency setup and verifies installed state,
so it reads 11 times the package bytes and writes two anonymous captures plus
one installed copy. These counts describe this fixture, not a claim that every
user operation has the same amplification. Source capture additionally performs
related before/after revision scans. Snapshot clones share disk storage.

The incremental requirements-only fixture additionally wrote 201,326,595 bytes
(192 MiB of comments/whitespace plus three delimiters) to one anonymous file.
The already-built test process completed in 5.25 s with peak RSS 31,404 KiB;
its UTF-8 carry remained at most three bytes and its buffer capacity stayed
below 131,080 bytes, with zero retained declarations. This isolates parsing,
including a huge newline-free comment, rather than measuring package capture or
dependency setup throughput. The file is reclaimed when the test owner exits.

## Independent review

An independent read-only subagent reviewed the implementation and requested three
repairs: numeric/container YAML expansion before allocation, availability under
an unused incomplete resource tree, and cancellable off-executor reload scans.
Follow-up review also checked scoped instruction evidence and collection-error
shadowing in the replacement domain fence. All findings were repaired and the
independent re-review reported no unresolved blockers. A final requirement audit
then identified raw observation work on unrelated huge files; the original
64 MiB observation budget is retained with complete accepted-domain evidence on
overflow, plus explicit unknown-file and accepted-large-content regressions.
The independent final review approved this repair with no remaining blockers. The numeric-array
reproducer (400,643 source bytes, 40,800,437 expanded bytes) now rejects during
preflight: its measured peak RSS fell from 709,556 to 27,904 KiB.

The subsequent PR review identified four further gaps: an impossible opening
Skill line could be buffered until newline, raw requirements comments were
retained, independent Workflow analyses could exhaust scratch capacity, and a
disconnected scratch parser could outlive request accounting. All four were
repaired and independently re-reviewed without remaining blockers. That review
also required preserving Storage classification for capture reads and compared
20,000 generated requirements preprocessing cases against the previous comment,
quote, whitespace and line behavior; all matched. No review agent edited code.

## Validation commands

The following complete validation ran before the upstream synchronization.
Rebased integration results are recorded separately below. Linux x86_64 local
checks cannot prove macOS-only execution; the PR requests the repository's
`full-verification` CI profile for that coverage.

| Command (working directory) | Result |
| --- | --- |
| `cargo fmt --all -- --check` (root) | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` (root) | Passed, including the final observation-budget implementation. |
| `cargo build --bins --all-features` (root) | Passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUST_TEST_THREADS=8 cargo test --all-targets --all-features` (root) | Passed: 5,897 tests, no failures; 20 intentionally ignored. |
| `cargo run --all-features --example check_test_lanes` (root) | Passed: native Linux CI lane coverage holds. |
| `uv sync --frozen`, `uv run --frozen pytest` (`test-support/fake-provider`) | 55 passed. |
| `pnpm check`, `pnpm typecheck` (`protocol/app-server`) | Passed regeneration/drift and strict reference-client contracts against staged generated artifacts. |
| `pnpm typecheck`, `pnpm test` (`dev`) | Passed; 42 tests. |
| `pnpm typecheck` (`tui`) | Passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 node --test --test-concurrency=1 'test/**/*.test.ts'` (`tui`) | 1,314 passed, 1 skipped, no failures. |
| `pnpm typecheck`, `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` (`web-console`) | 4,053 passed, 4 skipped; 270 files passed, 1 file skipped. |
| `pnpm check:i18n`, `pnpm check:provenance`, `pnpm build` (`web-console`) | Passed; 336 source records and notices for 349 production dependencies verified. |
| Fixed-image Playwright `--shard=1/2` and `--shard=2/2` (`web-console`) | All 635 cases covered: initial complete shards passed 341/342 and 291/293; after completion-wait repairs, both affected complete files passed all 10 cases. Distinct Host ports, one worker per shard. |
| `python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v` (root) | 64 passed. |
| `python3 -m unittest discover -s benchmarks/runtime -p 'test_*.py' -v` (root) | 37 passed. |
| `git diff --check`, `git diff --cached --check` (root) | Passed. |

Initial runs are not counted as passes: an old v69 local binary was replaced
before native WebUI validation; the coalescing test's single-poll assumption was
replaced with an explicit physical join observation; large-save/visual-save/browser-notice assertions now wait for the native
operation lifecycle rather than assuming five-second latency; and the variable-picker fixture selects its initial node by pointer
before testing keyboard positioning. The default parallel TUI run hit its
existing native-child observation guard under concurrent load; its exact rerun
passed, followed by the complete serial run above. The first 32-thread Rust run
had the repaired coalescing failure and an existing 64-round MCP anti-hang guard
failure; the unchanged MCP case passed alone in 8.29 seconds. The final complete
Rust run uses eight harness threads with every deadline unchanged. Its 20 ignored
cases are 12 opt-in measurements, one fixture generator, one git-lfs-specific
check, five live provider cases and one external MCP profile; these are not
counted as passes. A protocol predecessor-version documentation assertion was
corrected before the final passing run.

Browser authority is the repository-pinned
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
The initial unsharded run was intentionally interrupted to run both complete
shards. A second-shard startup attempt failed while two local Vite startups
rebuilt the same Node native addon directory; it ran no tests and is not counted
as a pass. Its restart reuses the already-running fixed-image browser service
with independent Vite/Host ports, after addon build settlement. No screenshot
threshold, runtime limit or product policy was widened.

## Rebased integration validation

The branch was rebased after PR creation because upstream allocated v70 while
this work was in progress. An independent integration review confirmed retention
of Model Profile ordering, its type assertions, the Settings source-intrinsic
repair contract, and the native/wire generic document bridge. Historical Runtime
Client version references were preserved separately from the App Server bump.

- App Server v72 / Runtime Client v82: all-target/all-feature check and Clippy
  passed; protocol regeneration and strict typecheck passed.
- WebUI full execution: 4,055 passed, 4 skipped; typecheck, provenance and build
  passed.
- TUI full serial execution: 1,315 passed, 1 skipped; typecheck passed.
- Affected fixed-image browser integration: all 28 cases passed, including
  Workflow editing/execution/visual authoring and upstream Settings/Profile
  ordering and intrinsic repair.

After the four PR-review repairs, all-target/all-feature Clippy, binary/fixture
builds, protocol regeneration/drift and strict protocol/client typechecks passed
again. WebUI build, i18n and provenance passed. Complete WebUI execution with
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm exec vitest run --maxWorkers=2` passed
4,055 tests with four skipped; the native-addon build had already completed
through `pnpm test`. The complete serial TUI command above passed 1,315 tests
with one skipped, including the eight-check/two-wrapper/overlapping-read and
failed-callback recovery regression. All 31 cases in `workflow-branch.spec.ts`,
`workflow-editor.spec.ts`, `workflow.spec.ts` and `workflow-visual.spec.ts` passed
in the pinned browser image. The new six-reference/overlapping-edit case covers
both Global and Workspace scopes. These are affected-path rechecks on the
rebased head, distinct from the earlier complete 635-case catalog.

The pre-repair rebased eight-thread Rust run passed its library (5,196 passed,
10 ignored) and preceding integration targets but hit the existing subagent
hard-parent-death checkpoint auxiliary-audit deadline. It is not counted as a
pass. The first post-repair WebUI run passed 4,053 tests and hit two existing
deadlines (held Workspace post-write reread and Office conversion); the complete
two-worker rerun above passed both. Test deadlines and product limits were not
widened. A compile-only attempt was interrupted to include the final typed
Storage-error regression before restarting the complete Rust command.

The final complete command
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUST_TEST_THREADS=4 cargo test --all-targets --all-features`
passed all 21 harnesses: **5,907 passed, zero failed, 20 intentionally ignored**.
The library contributed 5,201 passing tests and 10 ignored cases. The previously
timed-out hard-parent-death case passed within the complete subagent target
(44 passed), and real managed Python execution/reload passed within the tools
target. Ignored-case categories remain those listed above. Final dev typecheck
and all 42 tests passed again. Final formatting, staged/unstaged whitespace and
protocol generated-artifact drift checks passed.

## Follow-up review repairs after 5404876

The second review identified four remaining P2 contracts. The follow-up uses the
same architecture and keeps App Server v72 (this PR is unmerged):

- `workflow/toolCatalog` now keeps the detached request owner through cached
  Python verification, holds its `LISTINGS` permit until physical exit, and
  inherits Host resource cancellation. The real Stdio EOF regression gates
  `read_prepared_state` **after** manifest identity validation and **before**
  source hashing in an already prepared environment. It observes request count
  one, no available listing permits and pending drain, then releases the worker.
- Complete check reports are anonymous, read-only native scratch content. Both
  full output schemas and Explain use report references and 64 KiB reads, rather
  than inline control responses. Report references cannot serve as candidate or
  source-write capabilities; they share connection capacity/TTL and preserve the
  ordinary numeric codec. No source/candidate transfer byte cap applies to these
  derived reports. A carrier unit regression reads and hashes more than 32 MiB;
  real Stdio checks a three-node Workflow with 300 optional string properties,
  each with a 4,096-byte description, with Explain off and on, and compares the
  **complete** output schema. Requests and responses include 1 KiB of envelope
  allowance in the below-1-MiB assertion. Sibling analysis reports retain their
  existing 512 KiB native/256 KiB core bounds; Tool catalogs retain 256 KiB pages.
- Missing `server.py`, missing `requirements.txt`, and both missing are rejected
  before asset capture. Three 1 TiB sparse-file cases assert zero streamed bytes
  through direct discovery, winning-candidate discovery (including invalid
  higher-priority shadowing) and accepted-domain evidence fallback.
- Analysis generation/query ownership reaches whole-operation queue admission.
  Deterministic tests use the production Session, port and document helper with
  a controlled endpoint: block the first analysis, enqueue/cancel 12 large
  generations, then Save. Only the active analysis and Save upload. Separate
  latest-query and authority-fence variants cover both Global and Workspace;
  another port sharing the client saves through the same queue. Workspace
  admission occurs before Host HTTP encoding and dispatch. Started work retains
  physical cleanup; superseded waiters never begin scratch.

The two failures in CI run 42034903967 were treated as failed validation, not
passes. Large Skill materialization now has a separate real-child process test
with ordinary provider timeouts, leaving the timeout-inheritance test's original
27-second guard and 300-ms inherited policy intact. The classification-history
case isolates primary-worker admission and waits for physical auxiliary
admission settlement after each retained result, as its neighboring capacity
case already does. Retained result publication precedes permit release and the
admission generation change; the old CI log did not retain the detailed Busy
cause, so this is a verified lifecycle race correction, not a claim of precise
log-level forensic attribution. Production admission behavior and deadlines are
unchanged.

Independent read-only review approved all four repairs, including the Workspace
admission/authority fence. The final command
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUST_TEST_THREADS=4 cargo test --all-targets --all-features`
passed **all 21 harnesses: 5,911 tests passed, zero failed, 20 intentionally ignored**.
The library contributed **5,204 passed, 10 ignored**; all **45 subagent** tests
passed, including the unchanged 27-second timeout guard and the separate large
Skill materialization case. Ignored categories remain those listed above. No
pending CI job is counted as passed. The final strict-emulator WebUI run passed **4,061 tests, four skipped**;
serial TUI passed **1,316 tests, one skipped**, with the additional exact-u64 report
assertion also passing in a focused two-case real-stdio recheck. Protocol, WebUI,
TUI and dev strict typechecks, native/fixture builds, formatting, all-target/all-
feature Clippy, generated-protocol drift, WebUI build/i18n/provenance and all
**42 dev tests** passed. The six final generation/query/authority admission cases
also passed through the production port/helper in both scopes. Linux CI lane
coverage checks passed for both `rust-contracts` and `rust-boundaries`; the real
Python cached-verification scenario is selected by the boundary lane, whose
prerequisites install Python/uv, rather than the deterministic lane.

The first follow-up Rust attempt was interrupted: its two scratch TTL fixtures
had used an operation helper that refreshed the lease, and rebuilding the test
executable while other self-exec fixtures were running invalidated that attempt.
The fixtures now adjust the test clock state directly. All compilation finished
before the final uninterrupted complete command above, which passed. The initial
Clippy pass also reported two test-only narrowing casts and a large test future;
those were corrected and final all-target/all-feature Clippy passed. No product
limit or existing deadline was relaxed.

The fixed-image browser recheck passed all 31 selected cases. Environment:
`http://127.0.0.1:5193/` plus the fixture server on 5194; Chromium in the same
immutable Playwright v1.63.0 image recorded above. Browser plugin not available;
regular repository Playwright was used. The browser and fixture processes were
retired after the run. The large-source test now additionally carries a complete
Tool schema larger than 1 MiB in **both** Global and Workspace, checks/saves via
the actual native/Host paths, rereads equal persisted schemas, and observes every
native WebSocket control frame within 1 MiB. The separate real-stdio test covers
Explain off/on and asserts exact-u64 report fields remain wire strings. Existing
mobile cases in the selected catalog passed; the new large-schema save case uses
1440×1000. Screenshots remain external temporary QA artifacts, not repository
references.

| Rendered QA check | Result |
| --- | --- |
| Page identity and meaningful app content | Pass: rustX Workflow editor in the selected scope |
| Framework overlay | Pass: none |
| Console/page errors | Pass: zero relevant errors |
| Screenshot inspection | Pass: saved notice and no-unsaved-changes state inspected |
| User interaction | Pass: edit node title → native check → Save File → authoritative reread |
| Complete output schema and record bounds | Pass in both scopes; no schema truncation |

## CI browser fixture follow-up

At commit `365042568`, CI run `42050953656` passed Linux contracts and
boundaries, protocol, lint, TUI, Web/Host and browser shard 1. Browser shard 2
passed 339 cases and failed the large-authoring case during its unrelated empty
Session navigation prerequisite: `session/settings` was still capturing the
already-installed large resource inventory when the shared 5-second route
assertion expired. The trace had not sent `session/attach` or `workflow/check`.
The HTTP 400 followed fixture shutdown and is not evidence of a product failure.

The test now finishes empty-Session navigation before installing the two large
Workflow files, then opens the Workflow library and executes the unchanged
Global/Workspace load, check/save, complete source/schema reread and control-frame
assertions. No timeout, retry or product code changed. Independent review approved
the actual diff; strict WebUI typecheck and all **10 cases** in
`workflow-editor.spec.ts` passed in the same fixed browser image. The large case
completed in 42.0 seconds. This case covers large Workflow authoring rather than
imposing a five-second first-open Session inventory latency contract.

## Remaining limits and assumptions

External multi-file trees remain mutable: there is no claim of an atomic
filesystem snapshot. Complete content verification and frozen owned copies,
rather than timestamps or length markers, determine executable bytes. OS I/O
cancellation is cooperative between bounded chunks; a blocking syscall or
process/decoder shutdown must settle before its owner reports completion.

Full authorization/revision capture is linear in accepted content and can read
large unchanged assets repeatedly for related verification. Malformed/unselected raw trees
have explicit traversal and 64 MiB logical-byte observation budgets; an incomplete observation has a fresh, non-equal identity. Capture can instead
fence an independently complete bounded discovery-domain manifest, including
invalid candidate membership, higher-collection diagnostics, scoped instruction
bytes and every accepted content identity; if that
evidence is unavailable, capture fails. This preserves startup/repair for unused
malformed subtrees without treating a partial tree walk as equality. Model-choice paging
uses only authored configuration documents and does not rescan asset bytes.

Metadata/declaration parsing and Workflow in-memory graphs scale with their own
semantic content. Native Read retains its existing text-decoding behavior; its
50 KiB / 2,000-line model projection is not a source-memory guarantee. The pinned
MCP SDK's existing stdio and non-SSE raw-response buffering remains unchanged;
resource checks protect typed-content, decoded-blob and artifact allocation,
not arbitrary peer wire buffers. Workflow Code runtime values/commands keep
their 64 KiB execution bound even when the source document itself fits 16 MiB.
Scratch expires on subsequent carrier operations or connection close, with
admitted workers retaining ownership until physical settlement. Existing
prepared-environment retention remains store-owned. Available disk and I/O
errors remain real constraints on uncapped package admission.
