# Compaction presentation

Web and terminal clients present native blocking and asynchronous compaction
through one operation identity. The runtime owns admission, budgets, retries,
coverage competition, cancellation and checkpoint installation. Clients render
those facts and use the existing manual command and bounded readers.

## Native state and interaction

Each live operation has one updating status. Generating, retrying and foreground
waiting represent active work; Ready is a static notice and does not imply an
installed checkpoint. Committed operations leave the transient seat. Cancelled,
failed, superseded and no-effect outcomes remain distinct. Retained operations
from a previous runtime are labelled inactive rather than shown with a spinner.
Manual command results retain their exact request correlation even when another
operation completes.

Only an installed native Projection Head creates a historical summary marker.
Activation identity and the native transcript anchor place it among canonical
messages. Intermediate fragments and format fallback never create Chat entries.
Web reads one metadata page of 16 activations, with explicit earlier/later
navigation, plus the active head. TUI retains at most 32 visited heads and one
explicit `/context history [after-activation]` page. Both deduplicate by native
activation and preserve order when several activations share an anchor.

Web disclosure uses Enter, Space or pointer input. Expanded immutable text
survives transport reconnect; a changed authority, node or checkpoint cannot
borrow another summary or issue an obsolete read. A truncated preview retains
its limitation and uses the native checkpoint reader for expansion.

TUI displays the Pi-style boxed `[compaction]` indicator. `/expand compaction`
toggles the active checkpoint; `/expand compaction <checkpoint-id>` selects a
loaded historical checkpoint. `/expand all` and `/expand none` include these
markers. Existing tool expansion keys keep their current meaning. Bounded
preview text is terminal-safe and wraps within narrow CJK terminal cells;
`/context projection <checkpoint-id> [item-offset]` reads the full native
checkpoint through the existing inspection command.

`/compact [instructions]` remains idle maintenance. Busy admission never aborts
an Attempt. Instructions use the native 4,096-byte UTF-8 bound. A cancellable
foreground Attempt shows that Escape cancels that response. Detached background
generation has no client cancellation transport or invented Escape binding.

## Context and inspection

The context meter continues to show primary-model occupancy, its source and
window. An estimated rebuilt context is explicitly estimated. Auxiliary input,
output and reasoning usage belongs to its physical request; missing quantities
remain unknown, and do not increase primary occupancy or become zero.

The Compaction Inspector exposes bounded native Calls, accepted fragment count,
predecessor identities, Native/Prompt/fallback strategy, and physical requests.
Each physical request displays its frozen Provider, logical model, Profile,
provider-facing model, local terminal outcome and reported usage. Quantities
retain their exact unsigned integer representation. Explicit pagination stays
within the selected operation and frozen read cut; switching sources rejects
late results. Trusted input, private reasoning and intermediate summaries are
excluded from this metadata projection.
The current numeric page API admits JavaScript-safe offsets; larger native
continuations produce an inspection error rather than a rounded selector.

The read-only Trace integration adds the exact committed checkpoint header to
its existing inspection head. It searches the selected operation's completion
through the frozen journal cut, verifies the operation/checkpoint/activation
relation, and reads the finite immutable activation header. Ready and future
completion cannot provide this relation. The integrated contract uses App Server v82, Runtime Client v89,
SQLite schema 116 and Child IPC v40.

## Immutable references

Harness is pinned at
[`fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159`](https://github.com/deepseek-ai/deepseek-harness/commit/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159).
Pi Coding Agent is pinned at
[`850ea2eae2f39eb16ae14e1f09448eab6da2188a`](https://github.com/earendil-works/pi/commit/850ea2eae2f39eb16ae14e1f09448eab6da2188a).
[references.json](references.json) records exact Git blob and SHA-256 identities
for the 15 inspected upstream files, including licenses.

| Pinned source path | Local surface | Preserved interaction | Native adaptation |
| --- | --- | --- | --- |
| Harness `ui-chat/src/client/chat/CompactionCommandCard.tsx` | `ContextSeat.tsx` | One lightweight pending command, distinct settlement, fold committed command into marker | Exact native manual request receipt and independent operation identity |
| Harness `ui-chat/src/client/chat/CompactionItem.tsx` and `MessageItem.module.css` | `CompactionCheckpoint.tsx`, presentation `Message.tsx` and CSS | Grouping, 24px header, disclosure, sticky expanded header, spacing and hover-capability rules | Native checkpoint availability and provenance; no fabricated shadowed counts |
| Harness `ui-primitives/src/icons/artwork.ts` | `CompactionIcon.tsx` | Regular API/context and disclosure glyphs, 14px presentation | Three direct SVG glyphs; no global sprite authority |
| Harness `ui-conversation/src/client/skeleton/ContextMeter.tsx` and CSS | `UsageStats.tsx`, `StatDialog.tsx` | Ring, percentage, expandable detail, Escape/outside dismissal | Native primary occupancy and explicit estimate/unknown sources |
| Harness `ui-trajectory/src/client/trajectory-compaction-definition.ts` | `CompactionInspection.tsx`, `TrajectoryInspector.tsx` | One operation with inspection outside default Chat | Native Calls, physical request lineage, frozen source/cut and final checkpoint |
| Pi `modes/interactive/components/status-indicator.ts`, `interactive-mode.ts` | TUI `status.ts`, `ConversationView` | One updating work line, reason and concise outcome | Session-owned background work may outlive Attempts; no implicit manual abort |
| Pi `modes/interactive/components/compaction-summary-message.ts` | TUI `compaction-summary.ts` | Box, `[compaction]`, collapsed token label and expanded Markdown | Native activation/anchor, estimated token qualifier, `/expand compaction` |
| Pi `modes/interactive/theme/dark.json` | TUI `theme.ts` | Compaction background, label and text colors | Pinned OKHSL tokens converted by the reference pi-tui 1.5.0 converter; product dependency remains 0.82.1 |

Harness paths above are relative to `packages/client`; Pi paths are relative to
`packages/coding-agent/src`. Existing unrelated source inventory pins remain
unchanged. Web source inventory includes the new compaction references and
local dependency closure. Web and TUI third-party notices retain the applicable
DeepSeek and Mario Zechner MIT notices.

## Visual evidence

[Evidence](evidence/) contains collapsed and expanded Harness/rustX comparisons
for light/dark themes at 1440px and 390px widths, plus the Pi/rustX terminal
comparison at 80 and 24 columns. These are actual pinned component renders:
Harness uses its verified component, stylesheet and icon artwork with shared
theme/Markdown adapters; Pi uses its verified component with pi-tui 1.5.0 and
adapters for theme, default expansion key and empty extension transformers.
They demonstrate presentation, not upstream application or runtime race parity.
The product TUI continues to use pi-tui 0.82.1.

`native-multi-call-inspector.png` comes from the real native browser acceptance
case. The existing Trajectory references change only the running compaction
row's misleading completed label and its native state text; the exact pixel
bounds and image hashes are retained in `visual-deltas.json`.

Browser evidence uses the repository's immutable Playwright 1.63.0 image,
`bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`,
and Liberation Sans. Terminal evidence preserves the actual ANSI line arrays
in `terminal-comparison.json`. Runtime identity, races and lifecycle are tested
separately against the native App Server and controlled HTTP provider.

See [validation.md](validation.md) for the invariant-to-test map and results.
