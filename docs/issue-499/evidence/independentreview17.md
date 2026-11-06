# Independent review 17: capture order and public MCP recovery

Result: no confirmed blocking finding in the reviewed follow-up source. The two reported defects are addressed at their existing ownership boundaries. This is source approval for the frozen delta, not a final merge-verification verdict.

The reviewer authored none of this follow-up. Review and execution occurred in `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`, on uncommitted changes above `7e1997cbeb74768bfe52c02a9b997b6fc4ad8cc8`. Base remained `00978bbe3072b211efe835950f7f99666e1cc02d`. Repository AGENTS.md, DEVELOPMENT.md and tests/README.md were read before review. No repository files, Git state, PR comments or remote state were changed by the reviewer.

## Reviewed identity

Sorted POSIX path + NUL + ASCII file SHA256 + LF, followed by SHA256:

- Five changed production/test files: `190809225e7a819cee516b898225c416e19c15baceb6db1654130746ba4dbd0d`.
- Native closure: 1013 tracked Cargo/toolchain inputs and `src/`, `tests/`, `examples/` Rust files, `069885bc61fc6425a4ec0cf84d0cd88cb1ddec512c60a2df5fa09c0739e17333`.
- Independent inventory enumeration matches the root audit exactly, with zero omitted or additional Native paths. All 67 committed issue-499 evidence files at 7e1997cb remain byte-identical.
- Independent source record: `/tmp/issue499/review17-independent-source-check.json`, SHA256 `5051f859c18f2adf1c534e29ac26f97856d234ceeb1254c2ce7764b2aa39f8d0`.

Files:

| Path | SHA256 |
| --- | --- |
| `tui/src/ui/app.ts` | `2613f8e9d88a56dadeefd656f093fa43af3cbebd043c45f18eb3281e8722a840` |
| `tui/test/startup.test.ts` | `0eba1588047f295e708c56f29020a5573eccd0a2dddfcd8da3df7149448ca6ad` |
| `src/local_runtime/session_runtime_manager/configuration.rs` | `f7347f1a23fad330ca064803cd5ac184edd9277a9b95a1841bab130b55d6b7ec` |
| `src/runtime/conversation_runtime/configuration.rs` | `ad7eed62aa0e994e72534e5748067069a36e829787c9fedc4e562293a402a576` |
| `tests/scripted/app_server/configuration/mcp_readiness.rs` | `74e608b5bcb85ad5c61664b98de20bcbeaa1edc563f9ac7b3c89a481e5ea653e` |

## TUI ownership review

The defect was an ownership join occurring before the connecting capture had settled. Moving App-level binding and `cached.combine(targetDraft)` after final Session resync and the latest intent acknowledgement removes the intermediate P+R draft on which a withdrawal could incorrectly prepend Q. While those awaits are pending, cached P stays in its exact Conversation entry; the selected pending draft owns subsequent R and the connecting owner owns captured Q. Escape, explicit intent refusal and repair failure restore Q before R before any exact-Conversation join. Binding, clearing the connecting owner and forwarding an eligible capture contain no intervening await.

The reviewer followed actual input capture, Editor provenance, `#retainConnectingInput`, raw `ComposerDraft.combine`, Host preparation/caching and Session resync. The change does not construct string offsets, validate retained drafts during navigation, re-upload confirmed receipts or reinterpret the captured text as commands. Full captured `UserInputBlock[]` and explicit Queue intent remain intact. Ordinary successful queued dispatch sends Q once and retains the independent P+R editor; restoration is not an automatic replay.

The prepared Session is concrete observed state, used for running/Queue presentation and preserving the selected target through a transport interruption. It is not installed as the active dispatcher Session while repair is pending. Immediately before binding, the implementation checks the presentation lease, Host, connecting owner, transport closure, prepared Session retirement/release flags and identity in the Host attachment table. `resync()` can resolve after retirement without granting authority; the new final guard rejects that case. Reconnect checks the same original Conversation before allowing a later join, retaining old receipt ownership when a replacement returns a different Conversation.

Independent tests exercised the added before/after-attachment Escape and intent-refusal pair with a real confirmed receipt, final-repair refusal, prepared retirement with and without a capture, same/different-Conversation replacement, plus existing structured empty-tail, literal paste, Queue/Steer, draft isolation and unknown-outcome tests. Assertions compare actual wire `turn/start.params.content` and targets, including receipt position and zero `turn/cancel`, automatic message replay and re-upload where relevant. The fixture changes move explicit repair and intent acknowledgements to the new binding cut while retaining no-dispatch assertions; they do not add a generic success RPC driver.

## Native recovery review

The recovery reason belongs to `ConversationRuntime`, which owns the retained chronological failure. The new wrapper keeps the ordinary invalidated/Unavailable predicate and additionally recognizes only a retained explicit (`Some(refresh_sequence)`) failure for which `runtime_refresh_is_current` confirms the exact adopted source authority and physical-owner cut. The manager uses this wrapper at the existing explicit public Reconcile entry. This introduces no automatic retry scheduler and does not loosen ordinary authored adoption.

The lock order remains runtime coordinator state followed by capability state, consistent with existing runtime publication/adoption paths. The diagnostic is not cleared by this eligibility query. Existing commit ordering still requires a strictly newer explicit successful whole-batch sequence to clear a newer failure; older pending R1 success only reanchors that retained diagnostic to the resulting adopted authority. Authored authority replacement continues to reject stale completions/diagnostics through the unchanged current-authority validation.

The new public regression drives R1, R2 and R3 through App Server `ConfigurationReconcile`, not a private refresh helper. It holds an actual provider Attempt while R1 waits for publication, records newer R2 failure, settles the Attempt, confirms Ready/current epoch and retained failure, then gates the actual R3 listing through the public entry. Counts are startup plus R1/R2/R3 = four listings, one held provider request and zero MCP tools/call; R3 success clears the failure. The pre-fix failure receipt is retained separately by the author; the reviewer did not reproduce that red run by changing source.

## Independent execution

All commands below were executed independently on the frozen source; author green results were not substituted for execution.

| Command | Actual result | Raw log SHA256 |
| --- | --- | --- |
| `node --test --test-timeout=15000 tui/test/startup.test.ts tui/test/composer-app.test.ts tui/test/session-focus.test.ts tui/test/convergence.test.ts` | 201 passed, 0 failed/cancelled/skipped; exit 0 | `06e57458f09c762f769711e41d665d4f6fa0211b1fa04cc7a1475310f0692ff4` |
| `pnpm --dir tui typecheck` | exit 0 | `8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92` |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::configuration:: -- --nocapture --test-threads=4` | 96 passed, 0 failed/ignored; exit 0; 20.957 seconds wall | `bea4b087d296383fa9646b7a856068675d6b61467723c79345ff8ad5e46291cd` |
| `git diff --check` | exit 0 | no diagnostic output |

Native command environment: `CARGO_INCREMENTAL=0`, shared `CARGO_TARGET_DIR=/home/capi/Documents/codes/rustX/target`, `CARGO_BUILD_JOBS=4`, `TMPDIR=/run/user/1000/rustx-issue-499-baseline-tmp`, `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Cargo ownership was explicitly handed over after the author command completed and released after this command settled; no competing Cargo command was started by the reviewer.

The 96 native cases include the new public regression and existing MCP readiness, stale-authority, pending-authored-configuration and adoption-boundary coverage. The client tests use actual App/Host/Client with controlled protocol replies; they do not prove real Native/provider/process settlement end to end. The native manager fixture proves native public recovery and provider/listing counts; this focused run does not replace the full external real-process suites.

## Documentation and limits

The accompanying architecture, configuration, invariants, TUI and issue-499 changes describe the same delayed draft join and explicit public recovery reason. They keep the submission-scoped 15000 ms default, absolute native deadline, immutable Attempt and safe runtime publication contracts unchanged. No #504 Workspace-selection prewarming, connection handoff registry, UI warmup surface, compatibility mode or generic adoption shortcut is introduced.

The review does not certify future edits, complete local validation, current-head CI, macOS or the final Merge verification verdict. Existing 7e1997cb green jobs and its interrupted browser command are historical evidence only. The final follow-up must still pass the mandatory full commands and FULL CI/Merge verification on its own final head/base/merge identity.
