# Automatic sequential compaction validation

The implementation started from `origin/main` at
`d4645468282dc6df56015fd72d0ad442946694c5`. It replaces authored compaction
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

The following checks passed on the initial implementation base before the
local Linux filesystem failure:

- Rust 1.98.1: all targets and features compiled, including real executables.
- Context engine 31 tests, compaction pipeline 21 tests, runtime integration
  42 tests, and the durable Call-admission-frontier regression.
- Agent request preparation 15 tests and compaction control 6 tests, including
  live native progress followed by cancellation.
- The real HTTP provider-emulator tests for more than twelve accepted chunks
  with one Native-to-Prompt downgrade, explicit cross-Provider/Profile
  selection, ordinary Summary selection, and repeated compaction.
- Provider emulator: 58 Python tests; protocol, Web and TUI typechecks;
  245 focused Web tests; 199 focused TUI tests; 42 launcher tests.
- Web production build, i18n, 336 source-provenance records, dependency notices
  for 349 packages, 64 repository-metadata tests and 37 benchmark-controller
  tests.

The final manual-compaction, rollback-allowance, small-history overflow and
multi-compaction fixture corrections require a fresh complete run. The local
full-library run was interrupted when the host drive backing WSL filled and
the Linux filesystem became read-only; that run is not a pass. Final format,
strict Clippy, complete native lanes, generated-protocol drift, full clients,
real browser acceptance and macOS verification remain required CI gates.

The semantic corpus is maintained separately in
[`tests/fixtures/compaction-quality`](../tests/fixtures/compaction-quality/README.md).
No paid live-model evaluation has been run. Emulator results demonstrate
execution contracts and physical-call accounting, without establishing real
model summary quality or billed cost.
