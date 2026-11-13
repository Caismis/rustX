# Automatic sequential compaction validation

The original implementation was based on `origin/main` at
`a81d053f`. It replaces authored compaction
tuning with a model-derived policy and one sequential Summary pipeline. The
background operation lifecycle and final Web/TUI presentation are separate
changes.

## Main integration state (2026-11-13)

PR #531 is being integrated with `origin/main` at `c076354d6` from PR head
`881d99759`. The integrated contract is App Server v78, Runtime Client v85,
Child IPC v39, SQLite schema 87, Runtime authoring schema 12 and Runtime
Manifest v6. The model catalog remains schema 2 and Session catalog schema 19.
The integration retains shared writable Thread clients, instance bearer/JWT
authentication, Provider-owned protocol, Agent resources and Skills, and
automatic configuration application. It also retains model-derived sequential
compaction, removes authored context tuning and Summary output overrides, and
keeps accepted chunk progress and distinct cancelled/skipped Trace terminals.

Local integration verification passed with Rust 1.98.1 and the locked dependencies:

- All-target, all-feature Rust tests: 6,167 passed, 33 ignored, none failed,
  with the required provider emulator and eight test threads. The host's HTTP
  proxies require `NO_PROXY=127.0.0.1,localhost` and the same lowercase variable
  for loopback fixtures; remote proxy settings remain unchanged.
- Complete Web suite: 4,812 passed, four skipped. Complete TUI suite: 1,351
  passed, one skipped. Provider emulator: 58 passed.
- All 27 distinct browser scenarios in the compaction, Settings presentation,
  Settings ownership and Settings observation suites passed. The delayed-write
  regression holds Subagent limits while independently observing Model timeout.
  The changed Advanced screenshot was regenerated and checked strictly against
  the same pinned Playwright image digest used by CI. Local execution uses its
  cached read-only OCI filesystem through Bubblewrap, without host fonts.
- Generated protocol and editor schemas, Protocol/Web/TUI typechecks,
  translation/provenance/notice/icon/geometry audits, production Web build,
  formatting, strict all-target, all-feature Clippy and Linux test-lane coverage
  passed.

The verification recorded below belongs to the earlier revisions explicitly
identified there. A fresh FULL CI run must verify the integrated commit,
including native macOS and the complete browser shards; historical green runs
do not establish that result.

## Integrated-head CI follow-up (2026-11-13)

[FULL run 43362630300](https://github.com/Caismis/rustX/actions/runs/43362630300)
tested `fe632a154`. Linux deterministic contracts, generated protocol, TUI,
Web/Host/launcher and both native macOS jobs passed. The Linux boundary runner
received a shutdown signal during in-crate harness compilation after all its
external boundary targets passed; its remaining tests and coverage were not
verified by that job.

The browser shards passed 763 of 768 discovered scenarios, intentionally skipped
two, and failed three. Two failures were stale integration assertions: repaired
configuration now has schema 12, and Advanced no longer has a Context form.
Those assertions now use the current schema and an editable Model timeout form.

The 200 MiB STL trace showed a real optional-preview lifetime defect. The receiver
started its two-second preparation timeout when the original model arrived,
while the Worker waited for the first GPU-complete frame before receiving its
start signal. A slow first frame could therefore terminate the waiting Worker
and leave every drag rendering the full model. The two-second timeout now starts
with that signal; the original absolute import deadline still bounds the wait,
and cancellation or disposal still retires the Worker exactly once.

Local follow-up verification passed 394 geometry tests, including 22 Worker
lifecycle tests, Web typecheck, source/artifact audits and the production build.
All five scenarios in the two repaired Settings specs and the unchanged geometry
latency spec passed in the pinned browser. The 200 MiB case completed in 15.3 s,
retained 100 samples per orbit/pan mode, admitted a 12-triangle exact preview,
returned to the full 4,191,372-triangle model, and produced zero idle draws.
The existing 120-second limit, drag count, GPU settlement and disposal checks
remain unchanged; this functional pass does not establish the opt-in 50 ms
interaction target.

The eight deterministic regressions addressing the historical review findings
also passed: reasoning-output recovery and both reduction paths, Summary-only
history recovery, repeated source identity, and cold cancelled/skipped Trace
reconstruction. A fresh FULL run must verify the follow-up commit.

The subsequent [FULL run 43377502654](https://github.com/Caismis/rustX/actions/runs/43377502654)
passed all Rust, macOS, TUI, Web and Host jobs, including the previously
interrupted Linux boundary lane. The repaired Settings and 200 MiB geometry
scenarios passed. One reasoning-order browser case exposed an independent
teardown race: its product assertions and zero-request check passed, but a
configuration convergence read reached the Node proxy after its Workspace Host
had closed.

The affected Settings specs now use the existing page-aware cleanup helper:
drain intercepted responses, retire interception, close the page, then stop the
fixture. The helper returns the exact shutdown report so request-count checks
remain at the original boundary, without a return from `finally` that could
suppress a prior cleanup error. A barrier-driven localhost HTTP regression
proves this order; two error-path cases prove cleanup failures remain visible.
All 30 cleanup/browser-verification unit tests passed, as did Web typecheck.
The three affected specs passed all 15 executions across three pinned-browser
repetitions. A new FULL run must verify this teardown follow-up.

## Accounting and limits

For a declared context window `C`, `M(C) = clamp(ceil(C / 20), 1, 65536)`.
The primary admissible input is `H = Cp - Op - M(Cp)`, with equality admitted;
proactive pressure begins at `floor(9H / 10)`. Recent-original retention is
`min(ceil(15H / 100), 20000)` and never weakens protected fresh input.

Initial total Summary output is
`min(summary output capability, max(1, floor(Cs / 4)), max(1, floor(H / 4)))`.
A frozen explicit reasoning budget requires at least that budget plus 1024
visible JSON tokens. The admitted capability and Summary window must support
this minimum. The Summary input allowance is `Cs - M(Cs) - Os`.
Output-exhaustion recovery can grow total output up to the Summary capability
and half its window, with extra headroom for an explicit reasoning budget,
bounded by `Cs - M(Cs) - 1`. Primary input does not cap this recovery ceiling.
Each increase reprices the same unaccepted source against the smaller Summary
input allowance. The final visible Summary is checked independently against
the complete primary request; reasoning Profile and output strategy stay frozen.
Rolling and final-fit reductions explicitly request shorter visible content
while retaining the learned total output allowance. Both use the same bounded
output-growth recovery and complete-request repricing, preserving the original
operation budget, deadline and accepted source coverage. A larger retry that
no longer fits is rejected before its physical Call starts.

For a 256000-token primary with 16000 output and a 128000-token thinking
Summary model with 64000 output capability, `H=227200`, the trigger is
`204480`, recent retention is `20000`, initial total Summary output is
`32000` and Summary input is `89600`. Reasoning and visible JSON share the
total output allowance; the selected Profile is frozen.

Complete request prices include instructions, schema, prior accepted summary,
new source ranges, wrappers, System, Tools and protected/request-only material
where applicable. Summary output token counts are never added to primary
input estimates across models. Component estimates are labeled diagnostics;
they do not establish additive pricing or actual provider usage.

One capture admits at most 8192 source occurrences, 1 MiB per record and
16 MiB cumulatively. Each request carries at most 32 ordered UTF-8 ranges.
The normal accepted-fragment/start ceiling is the captured byte-progress
bound, capped at 256. Twelve exceptional recovery actions are separate from
normal work, and at most three input shrinks are allowed. A physical start
requires an admitted plan. The blocking deadline is fixed from original
admission at `min(300000 + 30000 * (bound - 1), 7200000)` milliseconds;
re-entry and retries cannot renew it.

The JSON object is at most 64 KiB, with at most 64 entries per section and
4096 UTF-8 bytes per text item. A typed Native format-feature rejection or
completed format-contract violation permits one operation-local Prompt
downgrade. Authentication, transport, overflow, refusal, truncation,
cancellation and storage failures do not authorize downgrade. Every physical
request remains separately settled and counted.

Accepted fragments advance only after validation. The final request must fit
and historical size must decrease before one atomic checkpoint, activation,
completion and receipt commit. Intermediate outputs remain Auxiliary lineage;
the preceding-call chain also preserves rejected and cancelled calls across
Fork. Primary occupancy and Auxiliary usage remain separate, with missing
usage components explicitly unknown.

Every fragment repeats its captured source and kind, including the earlier
Summary distinction, independently of generated carryover. Existing Summary-only
history remains eligible for bounded reduction when it can make strict
historical progress; fresh input stays protected. Native Trace records both
cancelled and skipped operations as terminal with their actual end time and
without inventing a successful checkpoint, including after cold reconstruction.

## Recorded verification

Verification uses Rust 1.98.1, the locked provider emulator and the repository's
FULL profile. The [PR checks](https://github.com/Caismis/rustX/pull/531/checks)
record formatting, strict Clippy,
Linux contracts and boundaries, full native macOS boundaries, generated v73
protocol, complete TUI/Web/Host/launcher tests, production browser inputs,
both real-browser shards and their coverage check. A selected, unfinished or
unexecuted gate is not a passing result.

The Linux deterministic-contract lane in
[run 42136608086](https://github.com/Caismis/rustX/actions/runs/42136608086/job/127627030583)
was cancelled: its unit/in-crate step was cancelled, and its pure-contract and
coverage steps were skipped. That run does not verify the lane. The later
[FULL run 42155602959](https://github.com/Caismis/rustX/actions/runs/42155602959)
completed every selected gate successfully for `c8d6b7f435dacfef3e4740f934ca7eec1ecdc3e6`.
That result does not establish verification of subsequent changes.

Focused Linux verification passes 223 Rust tests in the all-feature harness:

| Owner suite | Passing tests |
| --- | ---: |
| Scripted Context pipeline and runtime composition | 114 |
| Native Trace, including interleaved terminals and cold reconstruction | 79 |
| Manual compaction and receipts | 6 |
| Dual-model policy | 6 |
| Chunk planning and exact coverage | 7 |
| Operation budgets and retry bounds | 11 |

The Web trajectory and compaction-receipt suites pass all 143 tests with one
worker. Generated v73 schema/TypeScript, Protocol/Web/TUI typechecks, the Web
translation audit, `cargo fmt --all -- --check` and strict
`cargo clippy --locked --offline --all-targets --all-features -- -D warnings`
also pass. Node typechecks use the declared locked dependencies.
The production Web build, source/artifact provenance audits and production
dependency notice audit also pass.

Real HTTP conformance covers more than twelve accepted chunks, one authorized
Native-to-Prompt downgrade, cross-Provider/Profile selection, repeated compaction
and cancellation. The provider emulator has 58 passing Python tests. Source and
fixture hashes use canonical tracked bytes, independently of Windows checkout
line endings; actual request and provenance bytes retain their native authority.

The response-loss browser test waits for the actual MCP page before inspecting
a retained editor. Reopened Workflow Model selection waits for node visibility
and focus, dispatches Enter to that node and waits for its Inspector before
checking the retained schema. Browser acceptance uses the repository's production build,
native process boundaries and strict screenshot comparators.

The semantic corpus is maintained separately in
[`tests/fixtures/compaction-quality`](../tests/fixtures/compaction-quality/README.md).
No paid live-model evaluation has been run. Emulator results demonstrate
execution contracts and physical-call accounting, without establishing real
model summary quality or billed cost.
