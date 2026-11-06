# Independent source review: TUI reconnect draft ownership

Verdict: **Approved for this bounded source change; no blocking findings.** This is not a merge authorization or a claim that the next head's FULL CI has completed.

## Frozen source and scope

Reviewed read-only in `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`, on committed parent `f35449b8f64779dbfc503ab0f15be0fc9b8f9012`, base/main `00978bbe3072b211efe835950f7f99666e1cc02d`.

- `tui/src/ui/app.ts`: SHA-256 `be1c6fc225d1eefa1958df35557fba5fe4e02b0c669939e02557826c2ed24c98`.
- `tui/test/startup.test.ts`: SHA-256 `c0f3709dfb2eaad2743f2a6603b44fa6cf59ceff48eb0a456825bffb6943c133`.
- Exact reviewed two-file patch copied to `/tmp/issue499/review10-reviewed-tui.patch`: SHA-256 `a07492c7f2dd63b8a5d0ea6cca074357f436cec58aa459255eb1d3fdfdfbf8ad`.
- Two-file inventory: `af7c6a8e5166577434a9e0469e68a5d4670ed7589055920c0f9645324f32cd07`.
- Current cumulative repair inventory: 62 inputs, `96db4d00935a19584045aa4c870ef4d4bab987db24f76bf31deefef97dfe152f`; followup since d2a5: 30 inputs, `c1391a48ad196c7ac6dd6e2d207fb0c09c7bea6e21a196ec112aa2098d3f5c8f`.

I independently re-read and recomputed every listed source input, not just the aggregate strings. I also enumerated the full current Native recipe (Cargo.toml, Cargo.lock, rust-toolchain.toml and all Rust files under src/tests/examples). Its exact 1,012-input inventory and every byte hash equal the previous inventory: `ea1c7076d0b16fae90bdcc40d51f4fcec2385b2840b42c202bcad4071fc41177`. Native, Web and protocol production logic are outside this patch. The new TUI README paragraph accurately describes the reviewed ownership behavior.

## Ownership review

The previous bug was stale draft ownership, not an instruction to restore text whenever the editor is empty. `#bindSession(undefined)` caches the focused Session's draft before installing a temporary unbound one. Recovery now retains and reinstalls that **same ComposerDraft object**, and the reconnect-only `previous` path in `#focusSession.beginConnecting` keeps that object rather than allocating a second draft. `editor.onChange` therefore writes P, Q, explicit empty text and Send clearing to the same object later selected from the Session/conversation cache. The unchanged `if (connection && typed)` attachment step no longer revives an old P when typed is empty: the cached draft itself is already empty.

The `previous` argument is supplied only by the private reconnect path. Ordinary A-to-B navigation retains the separate temporary connecting draft and Session/conversation-keyed cache. The added actual-app A-to-B-to-A-to-B test verifies both independent cached texts and exactly two initial attachments, without execution, detach, unload or cancellation.

Queued submission still captures a string once, sends `submissionIntent` for the exact known Session/node, and forwards it only after attachment/observation and intent acknowledgement. Forwarding uses a temporary submission draft; it does not consume the retained editor draft. R typed after Send and S typed while inbound acknowledgement is held survive both boundaries. Existing host, presentation lease, withdrawal, disconnect, quitting and authority guards remain intact. No catch path resends an old unanswered mutation; the tests retain the old uncertain request and prove only the explicit new Send dispatches once.

## Deterministic evidence

I independently ran, on unchanged before/after file hashes:

`node --test tui/test/startup.test.ts tui/test/composer-app.test.ts tui/test/session-focus.test.ts`

Result: **95 passed, 0 failed, 0 cancelled, 0 skipped**, 2.533771578 seconds. Receipt `/tmp/issue499/review10-independent-focused.result.json`; raw log `/tmp/issue499/review10-independent-focused.log`, SHA-256 `672e6f6d90ef38740b3131a6fdc2e4f966b8f506da8c253424ead111156bae44`.

The four new controlled reconnect cases exercise the real RustxTuiApp and editor with FakeTransport at the wire boundary:

| Case | Exact new effects and final local state |
| --- | --- |
| Send P | One attachment, one exact-node intent, zero turn/start while intent ACK is held, one exact P turn/start after release; final editor empty. |
| Replace P with Q and Send | Same one-owner counts; only Q is dispatched; old P is not restored. |
| Clear without Send | One attachment; zero intent and zero turn/start; editor remains empty after attachment. |
| Send, then type R and S | One captured P request; R survives attachment/intent ACK, S survives inbound ACK; final editor is R+S. |

All four retain one old unanswered turn/start on the old transport and prove it is not replayed. Existing ready/authority reconnect cases now also assert the accepted new input leaves the editor empty. Separate A/B cache coverage is an execution-free navigation control, not a provider-request proof. No sleeps establish these orders; exact pending wire requests and acknowledgements do.

I inspected original red-final receipt/log: six selected cases were **1 passed / 5 failed**, exit 1, log SHA-256 `caa6c364e9acb264cef0b933a0e3d6334e3558736f1e79bcf160deb5a10655a8`. P, Q, clear, and existing ready/authority final-empty assertions failed with stale P; the R/S case already passed and remains a useful control. The three intermediate A/B fixture attempts were nonzero and are retained; they are not passing evidence. The final A/B fixture aligns durable node identity and explicitly answers selection metadata before independent footer/resync reads. Its settled receipt is 1 passed / 0 failed, log SHA-256 `f454aaf9f37faa820b9fbc51641150b98ddba45562d1143de6bf0a444f50769d`.

I also verified completed parent-run receipts against their actual raw logs:

- `review10-tui-typecheck`: exit 0; 4.274956021 seconds; log SHA-256 `8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92`.
- `review10-tui-full`: **1,214 passed / 0 failed / 0 cancelled / 1 platform skip** (1,215 tests), exit 0; 39.493049906 seconds; log SHA-256 `5f3d3d1577eef9a22b89a31597e8bed206269b4a766a6b368d67be71d258538e`.
- `review10-tui-focused-final`: **95 passed / 0 failed**, exit 0; 2.513682749995496 seconds; log SHA-256 `08aa23c76ab3bd3a924eb468032b31f512276bdf7d7d7a9d5a46f21c7d837330`. This parent rerun corroborates, rather than replaces, my independent 95-test run.

## Verification limits and historical CI

The independent run above is a deterministic client ownership/wire-order proof, not a real Native/provider integration run. I did not independently execute full Cargo or browser suites for this correction. Parent-run full validation and the new final-head FULL CI/merge verdict remain separately owned and must be reported at their actual terminal states.

Fresh read-only GitHub precheck established historical head f35449b8 / actual merge `85bcbac44cf6fa6dce684cba732bc25a9c7e2f3b`, CI #1526 run 40098806721 terminal success: 15 jobs successful, 3 unselected profile jobs skipped, including successful macOS full and Merge verification. Evidence is retained under `/tmp/issue499/tui-draft-resurrection-precheck/`. That CI predates this correction and is **not** proof for a later head. PR remained open/unmerged with auto-merge null; main remained 00978bbe at that precheck cut.

No repository file, PR, issue or commit was changed by this reviewer. No #504 prewarm feature or compatibility path was introduced.
