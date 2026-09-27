# Issue 420 ownership audit (before implementation)

Base: ad863a24cf48fbb0d5182746e1046d4b64e8c167 (origin/main, PR 422 merged).
Primary worktree: main at that SHA, only untracked `.playwright-mcp/`.
No nested AGENTS.md applies to the affected paths.

- Native linearization: `runtime_client/projection.rs`, host state mutex,
  decimal u64 cursor scoped to runtime incarnation / Conversation. Bounded replay.
- Canonical: ConversationStore Message Ledger and transcript; transient:
  `attempt.in_flight`, publication frames, foreground assembly. Neither is recovery input.
- Snapshot: host `snapshot_with_trace` repairs pending and transcript, then
  `materialize_trace` decorates responses/statistics/occupancy and independent Trace.
  Those decorations currently have no complete event payload.
- Web: `AppServerClient.receive` refreshes every event; pending changes separately
  call `rereadPending`. `performRefresh` loops a dirty bit and replaces SessionView.
  Existing generation and exact AttachmentTarget fences must remain the routing owner.
- Planned incremental owner: UI-independent client reducer over native DTOs; exact
  routing/cursor/recovery stays in the existing client attachment lifecycle.
- Chat: `ChatViewport.restore` has direct writers from mount, update and observer.
  Planned automatic writer: one queued frame, latest user intent, retained semantic anchor.
- Message: transient row is a different parent/type from canonical Message, even
  though both expose message-id anchors. Unifying keys alone cannot fix remounting.
- Subscriptions: ConversationLive/Docks subscribe to SessionView; Totals subscribes
  to the entire snapshot. Planned boundaries: transcript/control, docks, stats, Trace.
- TUI: presentation reducer already handles event vocabulary; `attempt_settled`
  additionally queues a full snapshot for native completion/statistics. Its normalized
  presentation state is TUI-specific; the wire read-model reducer can be shared
  without sharing UI state or introducing a package.
- Protocol baseline: App Server 24, Runtime Client 49. Schema/generator/drift,
  fixtures, Web and TUI imports and negotiation must advance together if changed.
- FirstSubmissions owns text, Files, receipts and continuation. Native creation ACK
  remains navigation commit. Summary invalidations remain native catalog authority.

Reference inspected read-only: deepseek-harness
477b4f420553e8a52c2fbccc464d7561b239c443. Relevant sources:
`packages/client/ui-chat/src/client/chat/{ChatView.tsx,use-chat-scroll.ts,
use-scroll-follow.ts,use-chat-viewport.ts}`. Follow intent, clamp attribution,
semantic row anchors are relevant. Smooth native scrolling/multiple imperative
writers and Harness runtime authority are not adopted: this issue requires one
frame commit owner over rustX native identities. Existing provenance pin is unchanged.

The detached `rustX-issue-420-baseline` worktree retains the exact executable base.
