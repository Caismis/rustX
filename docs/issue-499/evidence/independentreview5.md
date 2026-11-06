# Independent source review — PR #503 cold navigation, reconnect and observer follow-up

Reviewed at 2026-10-24T10:20:07.933434+00:00 in `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`. The follow-up is uncommitted over `d2a5aff54e9e82df1680ad4887f6824cdb556976`; the owner-review anchor is `5a86a5631019ea0c7c16e12ed07bd92aa3869198` and base is `00978bbe3072b211efe835950f7f99666e1cc02d`. I authored none of these changes and modified no repository file.

**Verdict: approve the frozen source changes; no confirmed actionable finding remains in this follow-up.** This is source approval, separate from complete local execution and current-head/base/merge FULL CI/Merge verification. Historical failed runs are not converted to passing evidence.

## Independently verified identity

- Native: `ac3d1c568596d75dc6cc6982f6a4ed23d8097bac0cec08db711e7b4147244bf8` over 1012 inputs.
- This follow-up over d2a5aff5: `b9ceaa3e2a02164dd57b6050a5512bc047453a83edbfdf272b08cc77f60144bf` over 27 non-document inputs.
- Complete repair over the owner's 5a86a563 anchor: `3e23974ab31f8d2170e1dea305dd506372577ee3a7bbaf53944103f2f19eb634` over 59 non-document inputs.

I recomputed these values and compared every inventory path/hash to actual modified and untracked files. Native inputs are Cargo.toml/Cargo.lock/rust-toolchain.toml and all `.rs` under src/tests/examples. Non-document inputs exclude docs/ and Markdown. The common recipe sorts POSIX paths and hashes `path + NUL + lowercase SHA256(file bytes) + LF`. The final provenance update is included: it changes only the Composer treatment text and its correct actual local source SHA, leaving upstream provenance/license unchanged. The complete inventory and earlier source receipts remain distinct; packaging/documentation does not change these executable-source identities.

## Latest three owner requirements

**1. Web cold selection must expose the selected Session's own explicit Send.** I inspected App, ConversationSeat/Composer, the navigation machine, attachment-intake owner and first-submission/lifecycle boundaries. Ordinary Session selection, selection from Workflows/History and cold History Return now select a target-bound local `session-draft` before awaiting Attach. A changed editor destination gets the existing new binding/control identity; same-destination History Return preserves its logical draft. The active view is selected by the new Session ID, so it cannot borrow A's target, transcript, upload receipts or controls. Selection alone sends no submission intent. Explicit Send still uses the existing fresh Host authorization and exact-node receipt before readiness; upload/turn effects still wait for the actual attachment and its proofs.

Pending draft intake is bound locally without activating the registered intake registry. Thus selecting/typing/uploading local files for cold B cannot retire A's registered intake or consume A's in-flight receipt ticket. On executable B publication, the existing normal single-editor replacement rule applies. This is not a new cross-Session draft restoration store: previously discarded text is not restored on a later explicit return. Failure/cancellation retains B's currently visible local files, while later navigation cannot transfer them to A. Stale native ACKs remain owned by their original submission; they cannot clear or dispatch later B typing.

The new React cases cover Session, Workflows and History origins with A upload pending or A input ACK ordered before/after B; exact target/node/one-input counts, unchanged A target/Attempt, no cancel/detach/switch, separate filenames and no A intake consumption remain assertions. Failure/cancellation and fresh Host refusal are covered. History tests now require an editable cold Return with no receipt until explicit Send, followed by exactly one correct input after Attach. The new real-browser definition uses actual stdio B listing gates from warm A or Workflows, exact schema in the first request and zero provider/turn/B-upload before readiness; its A Attempt can be empty, while the React owner tests separately supply an active A Attempt and pending tickets. I do not conflate those proof scopes or claim execution from source inspection.

**2. TUI cold reconnect must accept a new explicit Send.** A negotiated replacement Host is installed in the app/dispatcher and watched before entering the existing cold-focus connecting flow; the closed Host is no longer retained behind an MCP-dependent attachment. The confirmed Session/node is resolved once and the editor's unsent text stays local. The connecting branch can dispatch the new exact submission intent even while recovery awaits Attach. Existing intent ACK plus real attachment/observation form the double barrier before the one queued continuation can send. Escape withdraws only local queued text; quit, replacement/loss and stale presentation leases prevent late forwarding. An old unanswered request remains unknown and is not copied into the new continuation.

Passing the previous Session to AppServerHost.attach only preserves allowed observation optimization: the unchanged session implementation requires matching Native authority, Conversation, runtime incarnation and applied cursor. A different authority receives a fresh native checkpoint; old target/schema state is not accepted by identity coincidence. Connection listeners retire the old watcher, and close/quitting/current-Host guards remain. New source tests cover ready, withdrawal, intent rejection, attachment rejection, disconnect, quit and replacement authority, plus an old selector that cannot redirect the new target. They require zero requests before the explicit Send, zero turns while Attach/intent ACK is held, exactly one fresh input for eligible success and zero replay of the prior unknown request. No Tool/model request is synthesized by reconnect.

**3. Delivery cancellation should prove the actual non-Attachment reserve.** The minimal repair explicitly counts 5 checkpoint owners + 8 Observe owners + 1 other control owner, with a physically parked delivery and two catalog metadata reads; the final delivery/cancel fills the eighteenth non-Attachment position. It no longer incorrectly equates this scenario with the full fifty-position transport envelope or manufactures 32 physical attach waiters. The separate admission/Host tests retain their full50/48+2 proof. This scenario keeps the physical read permit until settlement, fourteen unrelated parked owners, exact cancel and delivery terminal responses, exact-once response IDs, independent metadata release and post-cancellation checkpoint. The boundary name now states that precise invariant. No production capacity or timeout changed.

## Native lifecycle ownership correction

`prepare_runtime_catalog_observer` acquires and converts its observer guard to a resident resource while the composition is inert, before activation can admit recovered work. The returned future captures its watch and physical owner but is first spawned only after exact Loaded/result publication under registry and initialization-control fences. It creates no late semantic preparation admission. Activation/deletion/cancellation losing branches return and drop the unspawned future/guard; spawned observers check lifecycle and incarnation/Loaded authority before rebasing authored candidate bookkeeping. Publication/admission serialization and pending authored intent owners are unchanged.

The deterministic new test holds activation and then observer startup: the resident owner is already counted while runtime is unactivated; after Loaded, the held observer does not prevent exactly one native compaction admission; empty history supplies the expected error, with zero provider requests. Fixture close must then settle the observer. The existing compaction ownership test now prints its actual early admission reply if its entered channel closes. This eliminates a concrete ownership competition and improves diagnostics; it does **not** prove what early error occurred in a historical CI failure whose original reply was not retained. I make no retrospective root-cause claim for that failure.

## Previous four findings and fixture integrity

The prior d2a repairs retain their source owners: attachment has no generic Web30-second timer, exact acquired-transport discovery has no hidden second connect, finite attachment/metadata/control lanes keep new creation and existing submission reachable, and global connected readiness is independent of per-Session restoration. This follow-up adds missing actual cold-navigation/reconnect entry paths rather than altering the Native15-second submission clock, live validation, immutable Attempt, physical settlement, continuous authority or safe publication cut. It adds no #504 prewarm/lease/handoff/UI feature.

The actual six browser helper/callsite changes exactly match the previously reviewed scratch patch (`git apply --reverse --check` passed). Its target barrier uses a real delivered ACK on the current socket and latest request ID, handles full-target close and rejects lost/rejected/retired claims. Historical wire logs and original semantic/model/lineage assertions remain. My earlier controlled in-memory helper check was scoped to these same bytes; it was not a real browser run and is not presented as one.

## Verification limits

I ran fingerprint/inventory checks, read the complete actual production/test diffs and untracked tests, inspected relevant unchanged owners, and ran `git diff --check` and the read-only reverse-patch check successfully. I ran no Rust, TUI, Web or browser test suite for this review. Focused execution receipts belong to the implementation/validation owner. Full final validation is still required. Historical d2a Cargo5473/1/16, browser/CI1518 failures remain failed evidence; earlier source approvals and passing lanes cannot verify these later Native/UI changes. Final documentation/measurement/evidence packaging should be audited separately. Any change to a listed source input requires renewed inspection.

## Exact follow-up inputs

| File | SHA-256 |
| --- | --- |
| `src/local_runtime/session_runtime_manager/configuration.rs` | `c63de7dd72d256b8d89e8ec5c269e6a6b0b6126bc5974d06eb97d8b62a2a9d69` |
| `src/local_runtime/session_runtime_manager/lifecycle.rs` | `7f8d22a64207f371813c3690e88c0513bec97a1589fe9386d995a9ff6a4d4f33` |
| `tests/boundary/app_server_file_read.rs` | `b52487c0268ed1da2310b49ab7314603ce95c24279c4502c3ed2bf2698cf7e85` |
| `tests/scripted/app_server/compaction_ownership.rs` | `d33ab7566032ecf435549d9646ab2e6fdcfdc299d6f265c737108abd5a67d2b0` |
| `tests/scripted/app_server/mod.rs` | `7f410d44e6d24289a962d30ceec3d7140cb7e57f81557358ee283793e89d0583` |
| `tests/scripted/app_server/protocol/delivery.rs` | `6d2330c75b6a8dfdd62ee670d42848d5f73e24f29ca8659fcac958c7e1817b63` |
| `tui/src/ui/app.ts` | `d30418cbfb76eb6095f02554ee7cd890dd31a368ce914f43f11afd58ac45cb4e` |
| `tui/test/app.test.ts` | `e4704a57bbf0370c602b641e2acb378c03b269c71369c33b7f82989704b9bcf4` |
| `tui/test/startup.test.ts` | `f0976504cf4f78d34dab3cdb608e60bd63aff003cdebc5c08457e20ac4c4e3fc` |
| `tui/test/support/app-server-harness.ts` | `4ecaf0026743388e89a40c568b3f03e179e0b4795201fdc1b1e00c26d0092b33` |
| `web-console/source-inventory.json` | `0df891c3cd219f20c63eab60db1aaf598440bd92ff53ab518ea7c526a8df97c4` |
| `web-console/src/app/App.tsx` | `70d6f1fe976b59448d81a4f0cb1c7ff2299032b5e0a1e52a9536278b06fa5f4d` |
| `web-console/src/app/agent/ConversationSeat.tsx` | `510a8b47511d883ee70acb9b43dac40559ea3ea11577eb39d5140310af42d464` |
| `web-console/src/app/composer/use-attachment-intake.ts` | `5a469cb1b446977eebc97454e50ebdfece1bbadbea8fea7392733e6b741d5b03` |
| `web-console/src/app/navigation/machine.ts` | `d20504b81ee46e71108a9069d06946c811708d7e10dd6237a709b788aa6e7c53` |
| `web-console/src/app/new-conversation/ConversationComposer.tsx` | `e136facec8b0a86b71b7e32e47c71e88b81781aa257bb950b1bb6ca9ded37a22` |
| `web-console/test/cold-session-navigation.test.tsx` | `bf8470e6d6607ab7c88dceae53456027c4a03001b21bb7834d7f891134e0164b` |
| `web-console/test/e2e/composer.spec.ts` | `bd4a333233ea96fd00aa84a80cedc193c607e3a821588a1a74c652546822b3d5` |
| `web-console/test/e2e/live-model.spec.ts` | `ad0e1d7fb0ae5c88a7451b62ac63cd6a4a208f98626b3c54efeabb728103cc99` |
| `web-console/test/e2e/mcp-cold-navigation.spec.ts` | `5090aa73ad146244c0e486e12427ea86a763a73c10e989ae2c968a527c63ddb5` |
| `web-console/test/e2e/response-retry.spec.ts` | `1d04e91aff410f58aaaabb78a580ff7efc3599759c167e4f81041defb439d07f` |
| `web-console/test/e2e/shell-actions.ts` | `823926c851e519127b74c22df9b2814170df875ba14893a3ea945ffe6fdc81e1` |
| `web-console/test/e2e/sidebar-actions.spec.ts` | `1c5239f9981df03abf6f7e84d2c35bb581f7449f4d692749aacca612d5ed3481` |
| `web-console/test/e2e/wire-probe.ts` | `dbe0f585dc79e7a9474030d7092e76cc98a8f88faec5e044b976b91e21a9a012` |
| `web-console/test/history-draft.test.tsx` | `0f3c60be906af000f77a340deb3d8b7fa2aaf997ca3d216aa9f52d6de7cf9b48` |
| `web-console/test/history-navigation.test.ts` | `3c158ca864578b66632e912d17deab2b9ba94769ef089cc30e699b594b014cff` |
| `web-console/test/navigation-ownership.test.ts` | `190f067c60b0ee627844ac9e14d206b2738844526076d2bb363ec333c1739fa3` |

## First Composer observation-barrier reinspection — superseded

Reinspected at 2026-10-24T10:27:43.700633+00:00. Only `web-console/test/e2e/composer.spec.ts` changed; the other 26 follow-up input hashes and Native fingerprint are unchanged. I independently recomputed the refreshed 27-input and 59-input fingerprints above.

The completed first focused browser invocation is **22 passed, one failed**, and remains failed evidence. I independently checked the preserved trace archive SHA-256 `434e5378b12fe83370c38c57b4a877aea0da4a1a9894931609a50f9957c2da50` and its actual Frame results: `call@916` contains two concrete Agent Status identities with native message anchors, while `call@1323` returns an empty placement after reconnect. The test could therefore capture an intermediate empty transcript as its reload oracle even though transport, attachment and independent Goal/Todo state were ready. This is a precise observation-order failure; it does not justify swallowing arbitrary stale reads or increasing timeouts.

The new patch obtains the final Native checkpoint's status identity set using the actual shared `statusIdentity` projection, requires a positive set and waits for those exact rendered identities before disconnect. It records complete identity/anchor placement, then waits for that same placement after reconnect before taking the original reload oracle. Original reload count, exact placement, Goal revision, Todo, model/domain, geometry, no-browser-recovery-state and error assertions remain. No production file, native model request, sleep, timeout, ignore or error handling changes. The existing ordinary five-second assertion window is retained.

No actionable finding in this test-only ordering correction. The prior production source approval remains valid and the report is now bound to the final test bytes. A renewed focused run and complete final local/CI validation are still required; this review does not claim their results.

## Final Composer bounded-window and disclosure reinspection

Reinspected at 2026-10-24T10:40:50.647582+00:00. Only the Composer acceptance test changed again; Native inputs and the other 26 follow-up inputs remain byte-identical. I independently recomputed the final 27-input and 59-input fingerprints above.

**The preceding test correction was insufficient.** Its assumption that Native status history alone implied DOM placement was wrong: transcript and status history are independently bounded windows. My preceding approval of that test's barrier is superseded by this new inspection; it was never a passing browser-execution claim. The second focused invocation remains **22 passed, one failed**. No old failed invocation is reclassified.

I independently verified preserved trace SHA-256 `ea2eafb1da216fe780465d1689828cc8508467520143349349ef8f1924a4db91` and actual results: `call@920` contains both native identities/anchors; `call@1250` through the final sampled `call@1264` contain only the later post-tool annotation. AgentTranscript's source intentionally draws nothing for a status whose anchor is outside the loaded transcript, and its R03/R04 regressions explicitly prove this. Collapsed completed-Attempt annotations remain mounted; merely opening disclosure does not load an off-page anchor.

The final test now explicitly establishes equivalent presentation conditions before disconnect, after reconnect and after reload: use the real read-only Load earlier control until every exact Native status identity has its loaded anchor; open each exact Native completed Attempt's presentation disclosure; require each annotation visible. The owner key matches actual `attemptOwnerKey`/status grouping. It then compares all identities and complete anchors with the initial placement and preserves original reload count/placement/domain/geometry/no-browser-recovery-state assertions. Nothing is filtered out, missing anchors are not ignored, and no data/state is manufactured. Load earlier and disclosure are browser read/presentation actions, not model dispatch or history mutation. No production change, sleep, timeout increase, ignore or error swallowing is present.

This is a legitimate explicit-window reconstruction proof under the actual bounded read contract, rather than a claim that disconnected/reloaded latest-page windows must contain all historical anchors. The original earlier disclosure/default assertions remain. Source approval is valid for these final test bytes and unchanged production code; standalone/focused/full browser and final CI results remain separately required and are not claimed here.

## Final optional-field narrowing

Reinspected at 2026-10-24T10:47:20.310900+00:00. The sole further change aliases `settledSnapshot.statuses` and throws an explicit failure when Native did not retain status history, then uses that narrowed value for the same identity and Attempt maps. It supplies no empty fallback, filters no missing fact and weakens no assertion. Paging/disclosure, positive status count and complete identity/anchor comparisons are unchanged. No production code changed.

I independently recomputed the final Native fingerprint (unchanged), follow-up27 fingerprint and cumulative59 fingerprint above and verified that the other26 follow-up file hashes are unchanged. Source approval remains valid for these exact bytes. The final source table supersedes earlier Composer-test bytes; the historical failing attempts and their insufficient barriers remain disclosed above. Typecheck, renewed actual Composer/browser and final exact-head/base/merge CI outcomes belong to separate validation receipts, not this review.
