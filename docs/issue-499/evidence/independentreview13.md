# Independent review13: reconnect publication and input intent

Verdict: no actionable findings in the reviewed follow-up delta. The reviewer did not edit its production implementation or regression tests. This is a review of the frozen eight inputs below relative to `0b7196c846932ddf28fb7d2fee14267562904464`, not a fresh independent execution of the entire PR's Native lifecycle matrix.

## Preparation and exact scope

The dedicated worktree is `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`. A fresh fetch retained `origin/main` at `00978bbe3072b211efe835950f7f99666e1cc02d`, an ancestor of the inspected HEAD. No checkout reset, history rewrite, commit, push, merge, rerun or cancellation was performed by this reviewer.

Live PR503 remained open and unmerged with `full-verification` and no auto-merge. Issues499 and504 remained open with their superseding submission-wait and separate prewarm scopes intact. AGENTS.md, DEVELOPMENT.md, tests/README.md, the CI profile/gate owner and current v57 protocol-generation scripts were inspected. The preparation capture is `/tmp/issue499/review13-precheck`.

CI1530's success verifies only its original0b7196/base00978/tested merge163a0545. It does not verify this follow-up. The original terminal snapshot remains byte-identical at SHA256 `c798c5e563fcb38bfa8468d73281c1a900d85fd6409496441385c9710a47bac9` and was never rewritten during this review. The eventual new head still requires its own FULL checks and current Merge verification.

The eight-file identity is `0662ca51f2c036d4a8550b049234cfadb9a74c8f8b5f1c60b330ec1ca2889da8`. Its recipe is SHA256 over the sorted relative UTF-8 path, NUL, lowercase file SHA256 and newline for each file:

| Input | SHA256 |
| --- | --- |
| `tui/src/ui/app.ts` | `65bd1ae2719e0020d389fc4ae8f60743d951c4f6ceb38a693a11bb62d052aeec` |
| `tui/src/ui/composer.ts` | `598cc5759b4ed8dcfbf2b1cbb9ec623087428f857eefd53c2309ed2e6dc719bc` |
| `tui/test/convergence.test.ts` | `927c88a389a6c4f0b290a9de29f2560e0f98818baeaebd76d2f8fac948dfffa0` |
| `tui/test/startup.test.ts` | `b81a67b1a5d5de53860f64ef6605523d51baaa447e19f47b6d8a78278ff2e78b` |
| `web-console/src/app/navigation/machine.ts` | `a439163641b3f9f9af0dba0f1ce166c02f816f058dab16829b58e56dc8ad75a6` |
| `web-console/test/cold-session-navigation.test.tsx` | `419798569cd16f1dcda4075a49d1284b87508807c6e882ff34d4e2ec8385d882` |
| `web-console/test/e2e/mcp-cold-navigation.spec.ts` | `a397fbf137e942c578c252756bfe6750a524198eeb5d65945ecf441c8e67a2d2` |
| `web-console/test/navigation-ownership.test.ts` | `8d882cbb508118e5d18156ba5cdb9ba47bbd6dc00850b71a04035a7132984a8d` |

The per-file inventory is also retained at `/tmp/issue499/review13-independent-source-inventory.json`. Final TUI and Web command receipts capture before/after input hashes; all relevant inputs were stable during execution. The final browser-spec input is `a397fbf1…`, including its title, console/page-error and Vite-overlay assertions; the earlier5c8ff candidate is not substituted for it.

## Source findings and resolved boundaries

1. **TUI same-draft reconnect.** Unbinding the old presentation now explicitly retains the same `ComposerDraft` object. This avoids the temporary editor clear/replacement that discarded `ComposerEditor`'s private paste-command provenance. Ordinary Session changes and deletion still follow their existing replacement path. Actual pasted leading tokens and token-end extensions remain literal, including suffixes; a deliberately typed command token with pasted separator/arguments remains a command. Explicit clear or replacement retains its existing reset semantics.
2. **Known-unsent restoration.** Review found the adjacent clear-then-restore path could still turn captured literal `/settings` into a command after Escape or intent refusal. The final change adds a concrete literal-message restore operation and uses it only when previously captured Agent message blocks return to the editor. It clears the pending token-end decision and retains literal semantics. It does not globally change ordinary `setText`, replay dispatched/unknown mutations, or grant command authority from text. The two real-App regressions assert zero Settings reads and one exact later explicit input request.
3. **Queue choice and complete content.** The connecting owner freezes `{ content, queue }` together. Its content remains the complete cloned ordered blocks, including confirmed receipts and an empty tail. Both acknowledgements still gate a single consumption. The successful continuation forwards the captured Queue choice instead of hard-coding Steer behavior. Every close, failure and withdrawal path restores only known-unsent content; newer typing remains independent, and an unanswered dispatched operation is not replayed.
4. **Web logical selection versus carrier publication.** A still-selected cold Session retains its draft/local intake through same-authority carrier loss. The old invocation is retired, and a new generation-bound preparation must obtain fresh Native attachment/control proof. `SESSION.RESUME` checks the exact selected draft/intent, current connection generation and authority; normal XState invocation correlation and final observation checks remain intact. It does not restore old command or drawer-close interest. A cancelled selection, newer target or changed authority cannot be revived by old metadata or a manually delayed resume event.
5. **Disconnected Settings ordering.** Review also identified that reopening Settings after disconnect had settled could clear the logical draft owner because no preparation remained pending. The final predicate preserves only the still-selected logical Session draft, while the preparation fallback preserves that owner solely for carrier retirement under the same authority. Explicit cancellation/replacement still uses full retirement. Both immediate reconnect and separately settled disconnect/Settings/reconnect are covered through the real Client/Lifecycle and actual rendered App.

No new retry scheduler, RPC lane, Native startup policy, shared connection owner or #504 prewarm mechanism is introduced. Native readiness/cancellation/physical settlement and immutable Attempt owners are unchanged by this follow-up. The nearby documentation changes were checked for consistency with this client-only scope.

## Independently executed checks

Local runtime: Node24.19.0, pinned pnpm11.13.1. No `build:host` command was run by this reviewer, avoiding concurrent native-addon rebuild ownership.

| Command | Actual result | Wall time | Raw log SHA256 |
| --- | --- | ---: | --- |
| `node --test test/startup.test.ts test/composer-app.test.ts test/session-focus.test.ts test/convergence.test.ts` from `tui/` |157 passed,0 failed,0 cancelled,0 skipped; exit0 |2.898s |`0452c767fbd72d051a594b31af2a988c206dbedd3e07261f917a6b3f270a1f42` |
| `pnpm --dir tui typecheck` |exit0 |4.583s |`8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92` |
| `pnpm --dir web-console exec vitest run test/cold-session-navigation.test.tsx test/navigation-ownership.test.ts test/client.test.ts test/session-lifecycle.test.ts` |4 files,134 passed,0 failed; exit0 |8.969s |`36befb49ba99c998604a15ff1a268120f3f260d67d2599868b4c2ba3f5333f15` |
| `pnpm --dir web-console typecheck` |exit0 |39.140s |`8366207267355d3e3d5bf3bf6e8c94c5f93f6078c34f08973fa2b38cdda6cc92` |
| `git diff --check` |exit0 |recorded in receipt |no output |

Raw logs and command receipts are `/tmp/issue499/review13-independent-{tui-focused,tui-typecheck,web-focused-retry,web-typecheck}.{log,result.json}`. TUI counts were independently read from Node's default spec-reporter summary; the original command receipt's TAP-only parser left its counts object empty, so `/tmp/issue499/review13-independent-tui-focused-counts.json` records that read without rewriting the receipt. The two typecheck logs contain identical successful output bytes; their separate command receipts and before/after inventories establish their scopes.

The first Web focused invocation exited1 before loading Vitest configuration: a simultaneous owner-controlled `build:host` had temporarily removed `node-pty`'s native module. Zero tests ran in that attempt. Its original log and receipt remain `/tmp/issue499/review13-independent-web-focused.{log,result.json}`, log SHA256 `df3b981431812be2352e7a7af909aead6f1e182b38e6808b8bdb79cbd0cd25bd`. After a read-only `import("node-pty")` confirmed the module was available, the same focused command passed on unchanged final inputs. No source, assertion, deadline or error suppression was changed for this rerun.

## Verification limits

The157-test TUI run includes the26 new actual-App gesture scenarios and4 direct Editor scenarios, plus the existing structured draft, unknown-outcome, newer-typing and independent Session-cache controls. These controlled FakeTransport tests establish client input and wire ownership, not physical provider execution or cleanup. The134 Web tests establish actual Client/Lifecycle/DOM and stale-publication behavior over the controlled carrier. The real-browser MCP reconnect regression was inspected as source but was not executed by this reviewer. Cargo, full client suites, full browser acceptance and new-head CI are separate owners' execution evidence and are not claimed here. Their final verdict must remain tied to the actual subsequently committed head and merge.
