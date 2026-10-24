# Independent review 9 — current Trace ledger scroll boundary

Verdict: **approve the exact frozen one-file Chat test follow-up; no actionable finding.** Read-only source/trace/receipt review; no repository edits or test commands were launched by this reviewer. I did not author this Chat patch. My preceding implementation of the separate catalog_recovery fixture is not represented as independently reviewed here.

## Identity

Reviewed HEAD `6a3e13cc2487ec497d7d932baa2395e52ba79980`, worktree `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`. Only `web-console/test/e2e/chat.spec.ts` is modified (+7/-0), SHA-256 `39278b3f1fe189f8f87582cf829faada83d71deec1170eab815c4da7a370fc45`. `git diff --check` passes.

I independently recomputed current sorted byte inventories with the existing path/NUL/file-SHA/LF recipe: Native1012 is unchanged `ea1c7076d0b16fae90bdcc40d51f4fcec2385b2840b42c202bcad4071fc41177`; current one-file delta `5d9f769709eca3bb066f271d1355dd9bd33c8dc36fd171d560a9984a6ba37239`; cumulative62 from5a86a563 `19e47f5ef6539cfb21c6144de51e9fd856701b69cc164f1796d7e25fea25314e`; cumulative30 fromd2a5aff5 `4b105f2ce26c537f59f8337040103527a1b6df12e97e4f8bcdffb8c2ab016085`. No production, Native, TUI, protocol, timeout or attachment-key policy change is present.

## Original failure and owning boundary

Inspected the original preserved trace at `/tmp/issue499/review8-chat-scroll-failed-artifacts/chat-native-history-rich-s-bfa12--and-native-upload-receipts/trace.zip`, independently verified SHA-256 `9b705f79d7e81caa333a65b2261eeb2d6deb0c9779760172b7630654c7d429e9`. The real exact-zero failure remains a failure; the complete invocation was subsequently interrupted with exit130 and cannot be called a complete415 result.

Raw trace confirms old retained52-row ledger at164894.486, then actual replacement Attach reply164976.906 and Subscribe reply165001.589. DOM snapshots show the replacement14-row ledger at164999.830 and still14 rows while the final scroll-to-zero gesture occurs; at165248.800 it has52 rows and scrollTop896. Thus the earlier reading-range predicate could observe the previous attachment, while the gesture later acted on a different short replacement surface. This is an invalid test synchronization cut; it does not by itself establish a production reader-retention regression. I do not infer a source fix from an old same-head green run.

Production `ConversationLive.tsx` intentionally keys the live Trajectory by Session plus attachment ID. Replacement observation installs the native Trace head before marking subscription/control readiness, and initial Trace assembly subsequently provides more history. `TrajectoryLedger` starts following the tail, interprets onScroll against actual scroll extent, and only disables following when a real off-tail position is established. With14/52 rows its virtualized branch (`rows.length>100`) is not active. Those existing policies are unchanged.

## Why the new barrier is sufficient

The test captures the old ledger DOM handle before reconnect. It then waits for wire.attachedTarget(id), which is scoped to the current physical socket and delivered newest Attach ACK, and a Subscribe response matching the complete target. Those RPC observations alone do not prove React committed a new surface; the added `retainedLedger.isConnected === false` explicitly proves retirement of the retained node. The current enabled native Stop control additionally requires executable observation authority. Only after those conditions does the original locator measure the new ledger's actual scrollHeight-clientHeight>200.

That final range predicate concerns an actual replacement DOM pane with a real off-tail position, rather than an arbitrary delay, old cached history or predetermined row count. It need not await every possible page: after a nonzero range is established, the original explicit tail-to-zero gesture truthfully disables tail-follow during later assembly/settlement. The one retained DOM handle has bounded test-context lifetime and is released by existing page/context cleanup. No new background owner is introduced.

The original tail/zero gestures, running Request identity, controlled provider release, completed/Assistant-row expectations and exact final scrollTop===0 assertion remain intact. Upload readiness/exact receipt, canonical history, provider response, images/lightbox and URL-lifetime assertions are likewise untouched. There is no added sleep, timeout, retry, skip, swallowed exception or weaker final predicate.

## Actual execution scope

Independently compared both receipts to their real log bytes:

- `review9-chat-typecheck.result.json`: command `pnpm --dir web-console typecheck`, exit0,15.602729983977042s; log SHA `8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92`.
- `review9-chat-browser.result.json`: command `pnpm --dir web-console test:e2e chat.spec.ts`, exit0,48.97962556598941s wrapper; log SHA `24453f85de0dae454b4af95ad5e1fd096f542e23c0f7f4df357631da834f2a7b`. Actual individual case33.1s; actual Playwright summary1passed(41.2s).

These are inspected runner results, not tests launched by me. The interrupted old full command and exact failing trace stay failed/incomplete. The new exact-source complete415 browser run and final-head/base/merge FULL CI/Merge verification remain required; this report is source approval and focused evidence only, not merge approval or complete CI success.
