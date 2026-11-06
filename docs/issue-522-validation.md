# Automatic sequential compaction validation

The implementation is based on `origin/main` at
`a81d053f`. It replaces authored compaction
tuning with a model-derived policy and one sequential Summary pipeline. The
background operation lifecycle and final Web/TUI presentation are separate
changes.

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

## Recorded verification

Verification uses Rust 1.98.1, the locked provider emulator and the repository's
FULL profile. The [PR checks](https://github.com/Caismis/rustX/pull/531/checks)
record the required results for the final head: formatting, strict Clippy,
Linux contracts and boundaries, full native macOS boundaries, generated v73
protocol, complete TUI/Web/Host/launcher tests, production browser inputs,
both real-browser shards and their coverage check. A selected, unfinished or
unexecuted gate is not a passing result.

Real HTTP conformance covers more than twelve accepted chunks, one authorized
Native-to-Prompt downgrade, cross-Provider/Profile selection, repeated compaction
and cancellation. The provider emulator has 58 passing Python tests. Source and
fixture hashes use canonical tracked bytes, independently of Windows checkout
line endings; actual request and provenance bytes retain their native authority.

The response-loss browser test waits for the actual MCP page before inspecting
a retained editor. Browser acceptance uses the repository's production build,
native process boundaries and strict screenshot comparators.

The semantic corpus is maintained separately in
[`tests/fixtures/compaction-quality`](../tests/fixtures/compaction-quality/README.md).
No paid live-model evaluation has been run. Emulator results demonstrate
execution contracts and physical-call accounting, without establishing real
model summary quality or billed cost.
