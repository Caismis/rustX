# Independent review14: passive Web recovery and persistent TUI provenance

Verdict: no actionable findings in the frozen ten-file follow-up reviewed below. The reviewer did not edit its production implementation or regression tests. This source review and the independently executed focused checks do not replace the required full validation and exact-head CI verdict.

## Why the preceding review was insufficient

The preceding candidate was `3af1a52abb9cbf72389b8537253a78e254328115`, based on main `00978bbe3072b211efe835950f7f99666e1cc02d`. Its focused checks passed, but the subsequent complete 416-case browser invocation ended **415 passed / 1 failed**, exit1. The failure is genuine production behavior, not an obsolete oracle: `sidebar attachment ownership: replacement`, `shell.spec.ts:503`, expected no current C authority after a lost Attach acknowledgement but observed it.

The reviewer read the preserved error context and Playwright trace under `/tmp/issue499/review13-browser-failure-replacement/`. The replacement carrier created a fresh C-4 attachment despite the old unresolved effect. The automatic `SESSION.RESUME` preparation had reused explicit `client.attach`/Lifecycle OPEN before ordinary RECONNECT policy ran. That bypassed the existing unresolved-operation fence. The old shell assertion remains unchanged. Original full-browser raw log SHA256 is `651c4bea4575be6d1397f8ad3a154927ef39a8d99de69a873d498bd40601851e`.

A separate owner report identified TUI provenance loss after a refused cold Attach, a real Enter-opened selector, and reselection of the same Session/node/conversation. The minimal actual-App red run preserved at `/tmp/issue499/review14-tui-failed-reselect-red.result.json` was 1 passed / 1 failed on the preceding implementation. Literal `/cancel` became a command after a temporary draft activation cleared the editor. The failure occurred beyond the earlier Escape/intent-refusal coverage.

The old review13 report, its checkpoint and prior CI1530 terminal remain unchanged. Their limited recorded observations are not represented as complete correctness or substituted for this follow-up's verification.

## Exact reviewed inputs

Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`; branch: `codex/issue-499-cache-aware-mcp`. The reviewed implementation/test delta is relative to `3af1a52abb9cbf72389b8537253a78e254328115`. The ten-file identity is `464ad48a511aaa931822bbc1ff4957e34f72564854d604386266381c0d3aa2f7`: SHA256 over each sorted relative UTF-8 path, NUL, lowercase file SHA256 and LF.

| Input | SHA256 |
| --- | --- |
| `tui/src/ui/app.ts` | `ac6a9dcfe24cd5d0711ace493c6fee99166df56dab67ed66a9843f53bbf6d2af` |
| `tui/src/ui/composer-draft.ts` | `f8a704070e59dafa1ddba47f07320dd80ad9da299665e195e2fa6575ac488ae7` |
| `tui/src/ui/composer.ts` | `725956ac05b58d958326ecccdb9c9329b33f9d4beba377fdab52b3fbdf9a274c` |
| `tui/test/convergence.test.ts` | `d15ea7e7d8208a95f7467a0fa3ba36c0361dbbbfb30ebcc6471f8fe710056f46` |
| `tui/test/startup.test.ts` | `77a4dd778f0c70ad96d0a4771d207d436da46e4d40bb265013992bb9433b0cce` |
| `web-console/src/app/navigation/machine.ts` | `6722834a9b296f3c31dffbbc0ebc48f14bd9fba6736b1952849fee5529d7fd44` |
| `web-console/src/client/app-server.ts` | `67c001ed3f24003c48e27ed37326b6530f9f148a3a3c5069dc0c5d47fdd6eeb9` |
| `web-console/src/client/session-lifecycle/system.ts` | `728157ef0acc17b3d2fcb2a6b0b666147441d86c2411cd89b1ae8e332dfca9a3` |
| `web-console/test/cold-session-navigation.test.tsx` | `4e82c2c69887890120ccef5791348c8fef8e22a8b6ae1a34b3f8a5cedef6aa90` |
| `web-console/test/e2e/mcp-cold-navigation.spec.ts` | `1eb19bcc7978a3495d97eaac1cea3ab3eeafd8e31a18cb2fc01abaad2edef71d` |

All four independent command receipts contain the same ten-file inventory before and after execution. The complete inventory is `/tmp/issue499/review14-independent-source-inventory.json`.

## Owner and safety review

**Web recovery observes authority; it does not grant it.** The new query reads only the already-registered Lifecycle actor. It requires a connected current-generation active Open, the exact current intent revision, and wanted/attaching facts. It creates no actor, operation, request or claim. Both the navigation observer and the correlated resume guard require that admitted preparation or a fresh current control proof. A resumed pending publication is explicitly marked as recovery and follows only `waitForAttachment`; it cannot enter the ordinary `client.attach` branch. Existing explicit OPEN and RECONNECT guards are unchanged, including the unresolved/failure/release distinctions.

The existing active-operation/facts projection supplies the wake after ordinary Lifecycle RECONNECT actually admits work. Thus a healthy cold recovery can renew the selected draft's input interest before its MCP gate or Attach acknowledgement completes; the user's explicit Send can still arm Native. Final executable-route publication continues to require fresh target/control evidence. This change adds neither an automatic retry loop nor a second attachment owner. Lost ACK, known refusal and released intent remain unopened automatically; a later deliberate selection can still use the ordinary explicit Open path. Old metadata, cancellation, a newer selection and replacement authority retain their existing fences.

**TUI provenance follows the logical draft.** `ComposerDraft` now retains the literal-token flag and pending token-end offset with its text/blocks. One capture/activation seam saves the old draft's provenance and restores the selected draft's text and provenance without using the ordinary replacement operation. Same-object activation does not replace editor content. All direct draft replacements in the focus/bind/connecting/catch paths now use that seam. Captured known-unsent message restoration marks the restored draft as literal and clears a pending token-end decision; ordinary `setText` clear/replacement and `ComposerDraft.clear` still reset provenance.

This covers failed Attach followed by the actual selector and same-conversation reselection, rather than only a same-text catch assignment. A and B keep independent cached drafts, including A's confirmed upload blocks; returning to a live B reuses its existing attachment and restores B's own text. The captured `{ content, queue }`, complete ordered blocks, consume-once continuation and later typing boundaries are retained. Neither selection nor recovery automatically resends a refused/unknown request. The new snapshot/restore API does not infer command authority from the string's slash prefix.

No Native Rust lifecycle, configuration/startup policy, protocol, RPC capacity or #504 prewarm behavior is changed by these ten files.

## Independently executed checks

Node24.19.0 and pinned pnpm11.13.1 were used. The reviewer first checked that the existing node-pty addon loaded, then ran the following commands without any `build:host`, Cargo or browser command:

| Command | Actual result | Wall time | Raw log SHA256 |
| --- | --- | ---: | --- |
| `node --test test/startup.test.ts test/composer-app.test.ts test/session-focus.test.ts test/convergence.test.ts` from `tui/` |164 passed / 0 failed / 0 cancelled / 0 skipped; exit0 |3.159s |`e1acc8762a732dfc012c61161b011c2ba3f67680fa25f0a8c0967111840f11cf` |
| `pnpm --dir tui typecheck` |exit0 |4.766s |`8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92` |
| `pnpm --dir web-console exec vitest run test/cold-session-navigation.test.tsx test/navigation-ownership.test.ts test/client.test.ts test/session-lifecycle.test.ts` |4 files, 138 passed / 0 failed; exit0 |11.187s |`0e186ce07e32152f1322668a2b4481ed41c80f6f53ba58df9e2bb53b13de1a92` |
| `pnpm --dir web-console typecheck` |exit0 |17.234s |`8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92` |

`git diff --check` also passed. Raw logs and receipts are `/tmp/issue499/review14-independent-{tui-focused,tui-typecheck,web-focused,web-typecheck}.{log,result.json}`; before/after input hashes are stable in every receipt. The two typechecks have identical successful output bytes, while separate receipts establish their commands and scopes.

## Test coverage and limits

The 164-test TUI run covers six added actual-App reselection scenarios, one added pending-offset A/B Editor scenario, four migrated literal-restoration controls, and all existing startup/composer/focus/convergence cases. The author's narrow 11-case subset therefore includes four existing controls; it is not eleven newly added tests. The exact failed path uses public Enter, the real ResumeSelector keyboard action, matching Session/node/conversation evidence and a second explicit Send. It asserts one literal input request and zero cancellation, and the structured A/B cases assert exact ordered receipts/content, independent B text and zero replacement upload. Clear/replacement controls still deliberately dispatch their newly authored command. Existing Queue, new-typing and unknown-outcome controls also passed.

The 138 Web tests include three additional real App/Client/Lifecycle negatives for lost ACK, refusal and released preparation, plus a positive held-recovery Attach case. The negatives assert no fresh B claim or hidden Attach and allow exactly one later explicit user reopen. The positive case asserts enabled B input with the same draft/file before Attach, one explicit B intent with zero pre-ACK input/upload, then one exact B upload/input and no navigation-created extra Open.

The reviewer inspected the unchanged old shell oracle, the actual old failure trace, and the two real-browser recovery test sources. The author's final targeted browser receipt `/tmp/issue499/review14-web-browser-final.result.json` reports 3 passed / 0 failed for the original replacement case plus the no-input and explicit-Send MCP recovery cases; this is the author's execution, not an independent browser run by this reviewer. Its final raw log hash is `e49a500ab31660f69474cf3a19bec3b813d4b295587a533269e32eef9cae2a2c`. The intermediate new browser fixture's missing provider marker remains a failed result and is not substituted with this later pass.

Controlled client tests establish client/wire ownership. They do not independently prove real MCP/provider/process cleanup. The new real-browser explicit-Send source checks zero intents/turns/providers before the explicit Send; while MCP readiness remains held after Send, it checks one accepted intent and zero input/upload/provider requests. Releasing readiness permits one first provider request with the complete startup tool schema and exact B input. The no-input case still checks zero intents/turns/providers. Full 417-case browser acceptance, complete Native/client validation and the eventual new-head FULL/Merge verdict remain separate execution requirements. No result from CI1539 or the preceding 3af1 head is claimed as proof for these later bytes.
