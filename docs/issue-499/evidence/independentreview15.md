# Independent review 15 — final post-Clippy candidate

**Source verdict: approve this eight-input follow-up; no blocking findings found.** This is not a merge verdict or a claim that final-head FULL CI has completed.

I did not implement or edit any of the review15 TUI/MCP production or test files reviewed here. I authored earlier Web follow-ups and some historical tests in this PR; this report does **not** independently approve those earlier changes or the complete PR. I reviewed the actual working-tree delta from `31d2394fe935e545466f18f9ae8fac18083422cb`, including the new untracked stdio test module, then independently executed the focused tests below. No repository files, production tests or previous reports were changed by this reviewer.

Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`; main/base remains `00978bbe3072b211efe835950f7f99666e1cc02d`.

## Post-Clippy finalization and chronology

Recorded at 2026-10-25T01:37:56.904416+00:00. The original independent report `/tmp/issue499/independentreview15.md`, SHA-256 `4c1e92310c1075bc17cc195776fdfcfb4c09a7f12897bc279a0a504e1a374ac7`, remains unchanged and identifies its original bytes. A later real Clippy run rejected `.err().expect(...)` in the newly added test under `clippy::err_expect`: `/tmp/issue499/review15-native-clippy.result.json`, exit 101, raw-log SHA-256 `ef5dacf16713069a96c62c364aa03430a4243aaec130f24a3d9a039df4fdde62`. That failed run is retained as a failure.

The author changed only this expression to `.expect_err(...)`; no lint allowance was added. I independently reversed that single replacement in memory and obtained the exact previously reviewed file hash. All other seven delta files, all production bytes and the unchanged performance fixture are unchanged. The failed-result requirement, later `.to_string()`, physical cleanup assertions and all credential/projection assertions remain in the same order. This report binds the final redaction test SHA-256 `e74387c8501c44c609c92c5f97c7e3ef2eb6a344aaa3069880c4895d18adc459` and freshly executed Rust tests below. The unchanged TUI's earlier independent 179-case/typecheck executions remain valid for their exact two inputs; they were not mechanically rerun.

I did not run Clippy again or claim that the parent's pending full quality pipeline has passed. The exact failing test expression is removed; fresh Clippy, complete validation, measurements and final-head CI remain separate required verdicts.

## Exact reviewed inputs

I independently derived the delta paths from Git plus nonignored untracked files and rehashed file contents. The current eight-input aggregate is `3e4002cbc330829ba404e481cb078558ab30e05b17356b319bc0a527de267690` (sorted `path + NUL + ASCII file SHA256 + LF`).

| File | SHA-256 |
| --- | --- |
| `src/tools/mcp/connect.rs` | `fc17fda7eb7102f5115060fa843c09c31ba080396783c5c6ed8bcc450bbd5f72` |
| `src/tools/mcp/connection/catalog_tests.rs` | `34f191e9cb3e05caf335e3f94305c0ae3a75d4aa6a8b567dd33b1b37795d3b37` |
| `src/tools/mcp/connection/catalog_tests/redaction.rs` | `e74387c8501c44c609c92c5f97c7e3ef2eb6a344aaa3069880c4895d18adc459` |
| `src/tools/mcp/fixture/raw.rs` | `79797e1b486a57223ea4296bf2510d8ce88f981d4a553f2914c8416dd9c7e36a` |
| `src/tools/mcp/framing.rs` | `04a124335edcfe829434566d2066c64609239f02418fe96ec140c779e4173da7` |
| `src/tools/mcp/runtime.rs` | `457c96dccd89747775e9a5b23f2cc16a8ce835608f27276bcaed96feca848af6` |
| `tui/src/ui/app.ts` | `f6180810eb08de5187c2b725ddba461a037ab01690bc94eb0d5007f6a6e20de3` |
| `tui/test/startup.test.ts` | `30eab4a3e11d81e447ca1a80c94b9345ab6d882d2bf2034b6325bc3604da3b49` |

`src/tools/mcp/connection/catalog.rs` is unchanged, SHA-256 `2f17c6f724b9d43f7769faf165e7cee66610a3165f22248ac0b7193ea4b5f090`. I followed its three health-check consumers, its transport/generation/epoch validation, and executor result propagation to assess the changed shared diagnostic owner.

The independently rehashed full Native input closure is **1013 inputs**, `1ea25f55fe1012ad6f26e2bd1e718c427ef82c193659e279d29c6daedbc797ff`. It adds `connection/catalog_tests/redaction.rs` to the previous 1012-input closure and changes the other five MCP paths. This fingerprint is an identity binding, not a claim to have semantically rereviewed all 1013 inputs. The former `ea1c...` Native closure is not this candidate and its full-execution receipts cannot be relabelled as a run of these new bytes.

The source inventory `/tmp/issue499/review15-independent-source-inventory-post-clippy.json` has SHA-256 `2ad8784ce0c4861dad399a9d300175f8ee664fa229b1567df1ad9e8dc69dedb8`. Independent path sets, counts and hashes also match the authored audit for the 17-input cumulative scope since `0b7196c`, 71-input repair scope since `5a86a563`, and 42-input follow-up since `d2a5aff`; those broader identities are not additional independent code-approval scope.

## TUI ownership review

The decisive cut now occurs synchronously before the first metadata await: `bindSession(undefined, targetDraft)` captures A's complete draft under A's actual Conversation key, removes the dispatcher Session target, retires its presentation lease, and installs B's independent staged draft. Keystrokes or paste arriving while `session/read` is held therefore update B's draft rather than A's. Submit remains disabled until B's exact node is known; pending history and the later attachment do not authorize A input.

Previously bound drafts, including upload receipts, remain under `Session:Conversation`. Only a never-bound draft uses `pending:Session`. A healthy existing attachment or reconnect supplies an exact known Conversation; a conflicting live confirmation refuses installation and queued forwarding. After an old B attachment has been retired, its cached draft is restored only if the fresh native target confirms that same Conversation. A different Conversation receives the new unbound input without the old receipt. This avoids carrying X's receipt into Y after a refused reconnect.

The queue transfer remains single-use: Send clones complete ordered blocks, clears the source draft, and retains the captured queue gesture separately. Input typed afterward remains a different draft. At a matching cached B confirmation, already-captured Q is not appended back to its old draft; the tests prove Q dispatches once and a later Send carries only the older reserved receipt/prefix/tail. Known pre-dispatch refusal/withdrawal restores captured blocks in order. Uncertain dispatched work remains in the existing non-replay path. Command/paste provenance and explicit Queue/Steer meaning are unchanged.

Metadata/Attach failure retains the selected draft without rebinding it to A. Returning to A restores A; explicitly reselecting B restores B. The selector test's update is valid: synchronous unbinding really retires the first picker lease, so its old `onSelect` cannot attach a new identity. The replacement test first proves that rejection, then obtains a fresh picker and proves B remains selectable. It does not remove the original conflict assertion or silently retry it. Host replacement, presentation-lease and late metadata checks remain in place; late old-host metadata cannot activate the retired target.

The 8 typing/paste × metadata/Attach × A-upload/no-upload cases assert exact B-only input, then exact A text/receipt restoration and zero unintended cancellation, detach, unload or submission intent. Additional cases cover metadata/Attach refusal, bound X-to-Y crossover, retired B same/different Conversation, captured Q, and disconnected metadata. Existing structured reconnect, literal slash, both ACK orderings, newer typing, Queue/Steer and ordinary A→B→A cases remain in the independently executed four-file set.

## MCP diagnostic review

Connection creation resolves the captured `SourceCredentials` before constructing the recorder. Its clone shares that generation's resolved `Arc<OnceLock<...>>`; it does not consult a later environment snapshot. Both the original serde reason and full offending wire line are redacted **before** the 256-byte reason and 160-byte line previews are retained. This is essential for a secret longer than the preview. The tee continues delivering the original bytes to rmcp, preserves first-violation ownership, and does not change classification or protocol negotiation.

Runtime health, protocol-poison and transport-failure diagnostic exits now use the same redaction-before-1024-byte-bound owner. The invocation listing wrapper sanitizes every returned error variant; remote listing error text is sanitized before being bounded. Catalog rejection can therefore produce a safe generic failed Tool status/model-facing projection before any `tools/call` exists. The three existing catalog health checks use the corrected shared owner; the fix does not duplicate their checks in another layer. Successful tool definitions/results, request generation, epoch fences, connection acquisition, dispatch and physical retirement code remain unchanged.

Each new real stdio regression starts a healthy negotiated/listed transport, holds final invocation validation, releases a malformed credential-bearing line through an explicit cross-process marker, waits for the actual violation recorder, then releases validation. Cleanup is established **before** the secrecy assertions: `retire_and_close` succeeds, the recorded physical supervisor PID is absent (`ESRCH`), and the journal proves exactly one listing, zero `tools/call`, and one established generation. The tests examine the actual failed status, model-facing projection, acquired-discovery rejection, poison diagnostic, runtime health and framing evidence for both short and long credentials, with redaction marker and useful wire diagnosis retained. The release-file polling is an explicit marker barrier, not elapsed sleep used as a race assertion. Existing schema/epoch/invalidation/reconnect tests remain in the 19-case catalog run.

The architecture/invariant additions and TUI README paragraphs match these owner boundaries. The TUI description distinguishes unbound presentation from receipts retained under their exact Conversation; no new startup/prewarm mode or protocol vocabulary is introduced.

## Independently executed validation

The two TUI commands below ran against their unchanged final TUI inputs. Both Rust commands were freshly reexecuted after the lint-only test change against the final seven-input MCP observation set (including unchanged catalog.rs). Each independent receipt captures its relevant source hashes before and after execution, and I recomputed every raw-log hash. There were no source changes, failed cases, skips or cancellations in these four successful test/typecheck runs. This does not reclassify the separate failed Clippy run.

- `review15-independent-tui-focused`: `node --test --test-timeout=20000 tui/test/startup.test.ts tui/test/composer-app.test.ts tui/test/session-focus.test.ts tui/test/convergence.test.ts` — 179 passed, 0 failed, 0 skipped/cancelled; exit `0`, `2.7428204730385914` seconds. Log SHA-256 `75d321cde250c2cd4f3621a086a857ce811caa4389bc8d38176c985222148738`. Receipt `/tmp/issue499/review15-independent-tui-focused.result.json`.

- `review15-independent-tui-typecheck`: `pnpm --dir tui typecheck` — typecheck exit 0; exit `0`, `4.197272782039363` seconds. Log SHA-256 `8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92`. Receipt `/tmp/issue499/review15-independent-tui-typecheck.result.json`.

- `review15-independent-mcp-catalog-post-clippy`: `cargo test --lib --all-features tools::mcp::connection::catalog_tests -- --nocapture --test-threads=4` — 19 passed, 0 failed/ignored; 4757 unrelated tests filtered; exit `0`, `194.45414596202318` seconds. Log SHA-256 `efe55d0289879b6112e64a6e1ca65a50e0a7af1a841099e195ceb065f78f25c5`. Receipt `/tmp/issue499/review15-independent-mcp-catalog-post-clippy.result.json`.

- `review15-independent-mcp-framing-post-clippy`: `cargo test --lib --all-features tools::mcp::framing::tests -- --nocapture --test-threads=4` — 9 passed, 0 failed/ignored; 4767 unrelated tests filtered; exit `0`, `0.27651898097246885` seconds. Log SHA-256 `c0a99ffdd0df82529671d5c0ffb05748c0649fb08300a33b48cea2da2cd8029e`. Receipt `/tmp/issue499/review15-independent-mcp-framing-post-clippy.result.json`.

The Rust commands used the repository-pinned toolchain and `CARGO_INCREMENTAL=0`, shared `CARGO_TARGET_DIR=/home/capi/Documents/codes/rustX/target`, `CARGO_BUILD_JOBS=4`, `TMPDIR=/run/user/1000/rustx-issue-499-baseline-tmp`, and `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. They ran only `cargo test --lib`; this reviewer did not build binaries or start a second full validation pipeline. Cargo was released to the parent immediately after both focused runs.

I also checked the preserved author red receipts and recomputed their log hashes: TUI metadata matrix **4 failed/4 passed**, separate bound-Conversation assertion **0 passed/1 failed**, MCP short secret **0 passed/1 failed**, and expanded short/long secrets **0 passed/2 failed**. They remain failures for their recorded intermediate bytes; current greens do not rewrite them. `git diff --check` passed on the inspected candidate.

## Remaining verification boundary

Source approval is limited to this eight-input review15 follow-up and the focused executions reported above. Full current Native/all-targets, full TUI, fresh measurements and final-head FULL CI/merge verification belong to the parent and are not asserted here. The completed 417-case browser run audited separately belongs to historical `31d2394fe` plus its frozen Web inputs; it cannot serve as execution of these new MCP/TUI bytes. This review did not implement #504, introduce automatic retries, or merge/enable auto-merge. No old report or repository source was edited.
