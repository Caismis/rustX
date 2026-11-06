# Review12 independent structured reconnect draft review

Verdict: approved; no actionable findings in the frozen two-file change.

Reviewer `/root/final_architecture_review` did not author this structured-input change or its tests and made no repository edits during this review. Reviewed in `/home/capi/.codex/worktrees/issue-499/rustX`, on committed head `a82d7b56812c4ad5ec165ff7e9eb163e9f079eef` plus the following frozen structured-draft changes:

| File | SHA256 |
| --- | --- |
| `tui/src/ui/app.ts` | `5fc3830c2dce8df3117bdffea72e5e2e14bc9923d8fdaa1b327da47c20d4dc7f` |
| `tui/test/startup.test.ts` | `67001a4d331832fa60ccee3f951a8d2b7d135cc8b10f4aaf5ccce6bacb9dfeaf` |

Two-file identity digest: `0bc7f728c5f89a20656f15764f42198796646e436323d50ce1b8edb5baa13d68`. Recipe: SHA256 of UTF-8 concatenation of sorted `path + NUL + file_sha256 + newline` records.

The connecting owner now captures a deep clone of `ComposerDraft.submission(text)` as ordered `UserInputBlock[]`, including text before confirmed upload receipts and an empty or nonempty editable tail. Clearing the originating draft transfers that content once. An empty tail with structured blocks still creates the exact-node submission intent. The attachment and intent ACK boundaries retain the captured array; actual Send/Steer choice uses the adopted Session projection at dispatch. The shared submission helper dispatches captured content without reconstructing it through a temporary text-only draft or clearing newer typing.

Known-undispatched paths remove the queued owner before restoration, then restore captured blocks followed by the current newer draft. Intent rejection, attachment failure, withdrawal and transport closure therefore retain receipt identity/order without reupload or automatic replay. Stale callback guards and the cleared queue prevent double restoration. A not-ready/submitting/uploading refusal after attachment retains content and returns before the success notice. Once a turn has actually been dispatched, the helper reports failure/uncertainty and keeps local prompt history without automatically restoring or replaying the transferred structured input. Ordinary Session focus and its separate cached drafts retain their existing ownership behavior.

Deterministic regression mapping: four success cases cross held-Attach/ready-controls with empty/nonempty tails and assert exact full native request content, exact intent/turn counts and no second upload; one case gates both intent and inbound ACKs while newer text is entered and proves the next independent request excludes the prior upload; nine cases cross intent rejection/attachment failure/withdrawal with empty/nonempty tails and newer typing, asserting a single explicit later request contains the exact captured sequence plus the newer suffix. Existing P/Q/empty/newer-text, ordinary A/B cached drafts, composer and branch-focus tests remain in the focused run.

Independent execution:

- `node --test tui/test/startup.test.ts tui/test/composer-app.test.ts tui/test/session-focus.test.ts`: exit 0; started `2026-10-24T16:12:28.426029+00:00`, completed `2026-10-24T16:12:30.895461+00:00`; wall 2.469s. Log `/tmp/issue499/review12-independent-tui-focused.log`, SHA256 `21d50c0e881ab5c0a3f03fb8aa9b94a0762809c892f011a613ef2d6dbdb38c6c`.
- `pnpm --dir tui typecheck`: exit 0; started `2026-10-24T16:12:28.425191+00:00`, completed `2026-10-24T16:12:32.558734+00:00`; wall 4.134s. Log `/tmp/issue499/review12-independent-tui-typecheck.log`, SHA256 `8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92`.

The focused Node run passed 109 tests, 0 failed/cancelled/skipped, including all 14 new structured cases. TUI typecheck passed. Source hashes were identical before and after both commands. Raw `.log` and `.result.json` receipts under `/tmp/issue499/review12-independent-tui-*` are retained.

Scope and limits: these are actual RustxTuiApp/Host/Client paths with controlled FakeTransport RPC replies and real draft-authoring UI actions; they do not prove a physical upload, native process cleanup or provider execution. Confirmed receipt identities are supplied by the controlled peer. This review does not extend into unrelated ordinary-navigation draft policy, Native startup/cancellation, full TUI validation or browser acceptance. The simultaneous CI1528 run tests committed a82d7b56, not these uncommitted structured changes; a fresh final-head FULL run is still required after commit/push. This reviewer ran no Cargo and performed no commit, push, merge or auto-merge action.
