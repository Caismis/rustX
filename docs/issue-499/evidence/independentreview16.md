# Independent review16 follow-up review

Decision for the reviewed source delta: no new actionable finding. The four reported failure paths are repaired in the inspected final bytes. This is source/focused-verification approval only: it is not approval of an unobserved final FULL CI or Merge verification result. Do not merge on this report alone.

## Identity and independence

- Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`.
- Branch: `codex/issue-499-cache-aware-mcp`.
- Previous published HEAD: `ec76eaf2131f5876eca17669d885bf5f864447c6`.
- Base: `00978bbe3072b211efe835950f7f99666e1cc02d`.
- Review is of the uncommitted review16 follow-up, identified by source bytes below, not a future commit SHA. The root must bind its eventual commit and fresh CI separately.
- I authored none of this follow-up's production, test or documentation changes; made no repository edits, commits, pushes or GitHub changes; and independently executed the focused tests below. I read repository AGENTS.md, DEVELOPMENT.md and tests/README.md.
- Root source audit `/tmp/issue499/review16-source-audit-final.json`: SHA256 `f8eccccd92b4b1cc3ff255a81741b9923f326bcaadabe4b5466ce4a675657697`.
- I independently recomputed all 14 current source/test entries and all 1013 Native inputs at the review cut. They match current-delta digest `aaa57d4b1f7312e9304837d091193f5250233bb1d075235a2ba1031bf9256799` and Native digest `a25e40e7636f406209fd5ef14f33218d3027bef8856d193f482169916902db3c` under sorted POSIX path + NUL + ASCII file SHA256 + LF.
- I independently compared all 63 `docs/issue-499/evidence` files committed at ec76eaf2 with their current bytes using git show. All 63 remain unchanged. Observation: `/tmp/issue499/review16-independent-audit.json`.

## Ownership and failure-path review

### Pending TUI input after prepared attachment / Subscribe failure

A Host's cached prepared attachment is no longer used to decide that a pending input draft was consumed. Ordinary selection activates a separate still-unbound pending draft before metadata resolution. Exact successful Conversation binding then combines the matching retained draft and pending draft. Deletion of the pending key is conditional on that key still owning the transferred object. Same-Conversation reconnect explicitly carries the previous draft; a changed Conversation is refused before combination.

`ComposerDraft.combine` does not mutate either input before installation. Identity/empty cases retain the existing owner. A nonempty join retains all visible text and receipts, including literal provenance, and preserves the original receipt-delimited ordering baseline. The actual App regressions include Host preparation success followed by subscription refusal, explicit reselect, captured Q and newer R, ordered upload receipts, independent Session A/B drafts, and an exact one-dispatch/no-cancel proof. Captured Q dispatch is separate from later P+R where the earlier successful Send already transferred Q.

I specifically checked the potential asynchronous upload owner hazard from allocating a combined draft. The current public Resume command route is rejected by the existing #uploading guard before command dispatch; its new actual App control verifies no new session/list or Attach while upload confirmation is unresolved. Reconnect carries the same previous draft object and takes the identity path. I did not find a reachable path that clones away an active upload completion owner.

### Navigation preserves invalid restored drafts without validating Send

Navigation and failure recovery no longer call `submission()` to ask whether retained draft content exists or to merge it. `retainedEditorBlocks` preserves raw ambiguous receipt-separated edits; it only applies edits to a single unambiguous text region. The full visible edit remains correctable. A new Q cannot turn an invalid P edit into accepted blocks. Both invalid-edited-P and legal-one-region-edit controls are present. Navigation succeeds without a spurious attachment failure; explicit Send still refuses the ambiguous edit with zero turn/start, and restoring the original order permits the same receipts without reupload.

Known-unsent captured content is prepended using the same raw-state approach, so a newer ambiguous edit cannot throw while recovering input. Existing explicit input-capture actions such as /attach still perform their own submission validation; the repaired rule is that restoration/navigation never do it.

### Same-generation Web observation recovery

The observer and guard now allow an existing selected cold draft to resume publication when there is no active navigation preparation and Lifecycle already owns current-carrier Open work or current control. That applies after explicit successful Refresh observation even when generation has not changed. The recovery preparation only calls `waitForAttachment`; it cannot issue a new Open/Attach or replay user input. Existing exact intent identity, target, authority, generation, cancellation and newer-selection guards remain.

The actual React App regression uses the real Refresh observation button after a held Attach and failed checkpoint. Current native B control becomes true and the still-selected B route becomes executable with the same logical text/file owner. It asserts exact A1/B1 Attach parameters, one original A upload and zero submissionIntent/turn/start/steer/cancel/detach/switch effects. Newer selection, cancellation and replacement-authority controls prevent obsolete publication. Existing lost-ACK / known-refusal recovery tests remain unchanged in the independently executed owner suites.

### MCP corruption actively poisons / requests shutdown

All three catalog health rejection gates now use `reject_unusable_transport`. It captures the proven sanitized unusable reason first, then invokes the existing protocol-poison operation. Capturing first prevents a violation observed at the diagnostic cut from being returned without the shutdown operation. Only a confirmed framing violation triggers protocol poison; ordinary transport-unavailable diagnostics do not manufacture one. No second transport acquisition, retry or new cleanup owner was introduced. The existing generation/connection retains physical settlement ownership.

The revised real-stdio tests retain a peer that ignores business stdin EOF and owns a background child. I inspected that `wait_for_settlement()` is passive: it cannot itself request shutdown. The tests capture production `closed` before any test poison/reacquire/retirement, wait for the existing driver's settlement and check the child PID is gone before explicit retirement. Explicit cleanup occurs before negative assertions so the pre-fix red does not strand the fixture. Per credential case: one established transport, one tools/list, zero tools/call, source credential absent from Failed/model-facing/health/framing diagnostics. The eventual registry has zero pending retirements. Both short and long synthetic-credential cases passed independently in the catalog suite.

### Actual failed Auxiliary CI gate

The prior CI1541 failure remains a real failure: the public `Auxiliary { reason: Busy }` did not retain the native rejection subtype or iteration. Nothing in this report reconstructs that missing fact or labels the failure flaky.

The result-delivery cut precedes Consumer permit completion, active-entry removal/generation advancement, and the reaper's lifecycle admission release. The revised retained-result fixture now waits for actual no-active-operation settlement before a distinct next call, rather than treating readable Delivery completion as full retirement. It parks the unrelated primary admission worker using the existing before-coordinator-lock Gate and a tiny concrete cfg(test) wake method. No production capacity, permit lifetime or lock semantics changed; no dummy inbound/provider request, Busy retry or sleep establishes the test ordering.

The separate deterministic test captures the next submission's generation, settles the first Call, then releases final transfer. It observes the exact `AuxiliaryBusy` message `auxiliary admission changed during preparation`, with provider count still 1. Only a later distinct submission after actual owner settlement produces provider request 2. The retained-capacity case confirms 16 retained results, exact Busy for the next submission, provider count 16, and a known command remains deduplicated after discard. These tests justify fixture owner synchronization without claiming they identify the original CI's unrecorded subtype.

## Independent commands and actual results

Every row is a new invocation by this reviewer on the reviewed bytes, not an author-result attribution. Raw logs and matching `.result.json` receipts use `/tmp/issue499/review16-independent-<name>`.

| Scope | Passed count / kind | Exit | Command wall span | Raw log SHA256 |
| --- | ---: | ---: | ---: | --- |
| `web` | 126 | 0 | 10.570 s | `f6cac21ef827b1754e5425259303f2c575e380620d47ebee4c8f7fb76fa0a970` |
| `tui` | 181 | 0 | 2.800 s | `da8b972d8c62d0b953fef1b2092adfb5804dc68474f369e7f66d3d74343577f3` |
| `tui-session-focus` | 11 | 0 | 1.066 s | `ba86e8c744dbee50ee371c8ef7d3c17cb2d338cf70a8b4eab6415cd7689eff18` |
| `tui-typecheck` | typecheck | 0 | 4.205 s | `8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92` |
| `native-auxiliary` | 10 | 0 | 0.951 s | `a253f8f5f7681a2886893ef3e750a33da45d7ebdb69d2c0280f9d03f4f287aae` |
| `native-catalog` | 19 | 0 | 0.743 s | `3b3a064c5c1f023087f7091f0eb59ac8d907d31a850fcc3aa36c9c6daa2e4048` |
| `native-framing` | 9 | 0 | 0.260 s | `b4f762f1041aab6f8a7cea66427a09454786dc382540a2a079f9b908b3428250` |

Web command: `pnpm --dir web-console exec vitest run test/cold-session-navigation.test.tsx test/navigation-ownership.test.ts test/session-product.test.ts test/session-lifecycle.test.ts test/first-submit.test.ts` — 5 complete files, 126/0.

TUI commands: `node --test tui/test/startup.test.ts tui/test/composer-app.test.ts tui/test/composer.test.ts tui/test/convergence.test.ts` — 181/0, then `node --test tui/test/session-focus.test.ts` — 11/0; 192 distinct tests total, zero skipped/cancelled. `pnpm --dir tui typecheck` — exit0. The native binary integrations were not selected by these commands.

Native commands: `cargo test --lib --all-features <selector> -- --nocapture --test-threads=4`, serial selectors `scripted_suites::runtime_client::auxiliary::` (10/0), `tools::mcp::connection::catalog_tests::` (19/0), and `tools::mcp::framing::` (9/0). Repository toolchain was used with `CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=/home/capi/Documents/codes/rustX/target`, `CARGO_BUILD_JOBS=4`, `TMPDIR=/run/user/1000/rustx-issue-499-baseline-tmp`, and `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. No Cargo command overlapped the Native author's Cargo slot. The six Native changed files were observed immediately before and after this serial execution and were unchanged (`review16-independent-native-source.json`).

`git diff --check` passed. An initial independent metadata wrapper had a manifest-key parsing error before starting any Cargo command; the corrected wrapper's three actual command receipts are the rows above. That wrapper error is not a product test failure or a skipped correctness test.

## Exact reviewed current source/test delta

| Input | SHA256 |
| --- | --- |
| `src/runtime/conversation_runtime/probe.rs` | `a27176ce0962e875a5cf5cd5221639329e39817a939b4f4333d0ab29e03b5096` |
| `src/tools/mcp/connection/catalog.rs` | `6bc8de1976127faaca8b1f55bc3e1e73eb6011899f4853f6bde764c9a8d63e69` |
| `src/tools/mcp/connection/catalog_tests/redaction.rs` | `b1615b712a92da3261bf667794a8305686b6b505f558bf8cc3c1b2650f9c095a` |
| `src/tools/mcp/fixture/raw.rs` | `a5e58e2e1bbd121324009169468aa8828dfb7871ea6c46c7a2838950f4f082a5` |
| `src/tools/mcp/runtime.rs` | `233f444e8c58f979ff0eeceeb4f06ad1aa19bc9a6bb1feb1092f72df73684552` |
| `tests/scripted/runtime_client/auxiliary.rs` | `0108687278affcc1fc224dc233af37106389d2e0d12848a310e76db4c8483f85` |
| `tui/src/app-server/editor.ts` | `53b41a57e9dde4db84fb58b75d4987ac0b224699e39e50925b214272e64959d5` |
| `tui/src/ui/app.ts` | `2dbd6779283f128723d88b55b27d9187b0dd3d53f5f263263a614d8992d05900` |
| `tui/src/ui/composer-draft.ts` | `3048761a44da5f0e61398c655e9694fe1153ad2cd0f78ee09ad8a6c6b1c821da` |
| `tui/test/composer-app.test.ts` | `e1102143198b05092a57966f4feac4ae83e39c7ae1605b82bb2ba409fb7f7ece` |
| `tui/test/startup.test.ts` | `6b58b58922ef9bf5f8a8d3cc73d194fd07c43528932332437481fae88b1e41ee` |
| `web-console/src/app/navigation/machine.ts` | `9516038a539fd4468db53167abd565e00297650d02ebab63198e59c4e818b94f` |
| `web-console/test/cold-session-navigation.test.tsx` | `7ccf8c51e7c9d8f10dac1622743f543ac1800280ec02af2a3eb210b9a4b66988` |
| `web-console/test/navigation-ownership.test.ts` | `a2fd027cebbc5813c0df76541463171878da40916f956f38bc73aa42621e4b71` |

TUI five-file aggregate: `341fab3f2a79ce8ed7896b3abff21a92bde9f40737779858ed5b6487daa28866`. Web three-file aggregate: `e1466052706858ff311616bf7ae54c4459ad96c54972bb3a63cef6088892086c`. Native six-file aggregate: `4b918bc8d92dd4693b44bbedf11d8f08138e111ad26beb4adeb6ebe076bb5ee5`.

## Scope and remaining verification

I reviewed the complete ec76eaf2-to-working source/test delta and adjacent owners: Session selection/reconnect and draft provenance/queue capture, Web Lifecycle publication, MCP validation/call poison/physical retirement, and Auxiliary consumer/reaper/admission. Architecture/invariants/TUI/issue documentation changes are coherent with these owners; a wording precision concerning explicit /attach input capture was reported to the root, without requiring a production change.

The Native submission-received absolute clock, 15000 ms default, CFG3 policy, immutable Attempt snapshots, automatic adopted-source readiness publication and pending authored configuration isolation are unchanged by this delta. No #504 workspace selection prewarm, handoff registry, UI or other new startup mode was added.

These focused results do not replace final all-target Native, complete TUI/Web/Host/launcher, emulator, protocol/lane drift, Playwright, platform and exact final-head FULL CI/Merge verification. Earlier ec76eaf2 browser417/0 and earlier green CI runs do not cover these new bytes. Root owns those remaining checks, ordinary commit/push and exact head/base/merge binding. No merge or auto-merge is authorized by this review report.
