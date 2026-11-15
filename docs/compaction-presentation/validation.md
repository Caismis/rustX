# Compaction presentation validation

The implementation is stacked on the asynchronous-compaction PR #562 and its
shared-kernel prerequisite PR #531. The original UI change is commit
`2e9aa65e10a8f6983f3f45b4395b3fba81aaf893`; the current adaptation retains the
current permission/resource policies, shared writable clients, native Thread
status, accepted submissions and request-freeze semantics.

## Invariant-to-test map

| Invariant | Owning evidence |
| --- | --- |
| One status per native operation, Ready does not spin, typed independent failures | Web `context-seat.test.tsx`; TUI `compaction-presentation.test.ts` |
| One committed marker, exact anchor/activation order, finite explicit history | Web `compaction-placement.test.tsx`, `compaction-summary.test.tsx`; TUI `compaction-presentation.test.ts` |
| Released text/focus survives reconnect; replaced authority rejects stale reads | Web `conversation-reading-disconnect.test.tsx`, `compaction-inspection.test.tsx`; real client reconnect acceptance |
| Exact committed summary, bounded preview limitation, no canonical summary Message | Web `compaction-trace-preview.test.tsx`; native Trace inspection and actual HTTP conformance |
| Frozen Provider/model/Profile, distinct physical fallback, exact u64 and unknown usage | Web `compaction-inspection.test.tsx`, real `compaction-chunks.spec.ts`; native Auxiliary conformance |
| Real multi-call operation in both clients | TUI `integration.test.ts` multi-call case and Web `compaction-chunks.spec.ts`: 21 accepted fragments, 22 physical requests, one final checkpoint, stable reconnect identity |
| Idle manual admission, optional instructions, typed no-op/cancel/error and no implicit abort | TUI command/receipt suites; Web receipt/context suites and `compaction.spec.ts` real native flow |
| Disclosure, CJK/narrow widths, resize and dark/light presentation | TUI compaction render cases; eight `compaction-summary.spec.ts` browser cases; pinned component comparison images |

The shared `chunked_compaction` HTTP scenario rejects the first native JSON
Schema request and gates the first Prompt retry. Both clients first observe
one native operation and no checkpoint marker, release that gate, then require
more than twelve accepted fragments and one installed marker. Web reads every
Call page and checks physical request metadata. TUI renders the actual native
state with its terminal component. Reconnect must retain the checkpoint identity
and one marker. No static fixture, timer or screenshot substitutes for these
native assertions. Test observation timeouts cover constructing the large
history; they do not change runtime/Auxiliary deadlines.

## Environment and verification

The current local environment is WSL Linux x86_64 with Rust 1.98.1, Node
24.21.0, pnpm 11.13.1 and the locked provider emulator. Real-browser checks use
the immutable Playwright 1.63.0 image filesystem and the repository comparators.
Build matching native executables before running client integration tests.

App Server v81 and Runtime Client v88 carry the read-only committed-checkpoint
inspection addition. SQLite remains 90 and Child IPC remains v40. Rust owns
schemas; regenerate the protocol pair and fixtures, typecheck all consumers,
and verify generation drift against the Git index.

The 15 immutable reference files in `references.json` have been re-fetched
through GitHub and verified by both Git blob and exact-byte SHA-256 identity.
The comparison images and terminal captures remain source-reference evidence
from the original UI change. The adapted implementation also passes 55 distinct
real-browser scenarios: 44 Trajectory cases, eight summary theme/locale/width
cases, and three native manual/multi-call scenarios. The ten Trajectory image
conflicts preserve the current layout and apply only the original compaction
role/state pixel delta after its row moved upward 30 pixels. Every changed pixel
was checked against its old source, six complete initial renders matched the
merge exactly, and all ten references passed the unchanged comparator in the
composed rerun. `evidence/stack-visual-deltas.json` records bounds and hashes.
No screenshot-update mode or comparison-policy changes were used.

The complete TUI suite passes 1,370 cases with one platform skip, including the
actual native multi-call/reconnect case. The large-history gate has a separate
five-minute preparation budget; subsequent generation assertions and native
execution deadlines retain their existing limits.

Required checks are formatting, strict all-target/all-feature Clippy, native
Trace and protocol contracts, real multi-call HTTP generation, complete Web/TUI
suites, all TypeScript projects, i18n, source and artifact provenance, dependency
notices, the production Web bundle and the full browser suite. The repository
FULL profile must pass every selected Linux/macOS/browser/coverage job and
Merge verification on the final PR head. The PR description records exact
local results and the final CI run; unfinished checks are not passes.

No paid live-provider quality, latency or billed-cost evaluation is included.

Current local checks also pass all four TypeScript projects, formatting, strict
all-target/all-feature Clippy, 82 native Trace cases, all 31 contracts, generated
schema/types, production Web build, i18n, source/artifact provenance, notices and
62 provider-emulator cases. HTTP Conformance accounts for 45 distinct passing
cases (four fixture/measurement ignores): one provider bootstrap raced the
initial virtual-environment installation, then passed after `uv sync --frozen`.

The full local Web run passed 4,915 cases, with ten failures under concurrent
compilation/client/browser load and four platform skips. All 333 cases in the
eight affected files then passed with unchanged five-second test limits. This
accounts for all 4,925 distinct applicable cases; the final FULL CI run remains
the authority for the complete suite on one head.
