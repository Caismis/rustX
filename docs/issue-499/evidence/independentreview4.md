# Independent final source review — PR #503 owner review round 4

Reviewed at 2026-10-24T09:09:23.408732+00:00 in `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`. Previous committed head is `5a86a5631019ea0c7c16e12ed07bd92aa3869198`; base/origin/main is `00978bbe3072b211efe835950f7f99666e1cc02d`. The reviewed follow-up is currently uncommitted.

**Verdict: approve these source changes; no confirmed actionable findings remain in this follow-up.** This approval is source review, not a merge or execution verdict. I did not author any of these code, test, protocol or documentation changes. I independently inspected the actual complete diff and the newly untracked tests before consulting the earlier scoped reviews; their conclusions are not a substitute for this review. Full validation and exact final-head/base/merge FULL CI and Merge verification are still required.

## Frozen source identity

- Native input fingerprint: `1de0fab54d77028c602d9d37d623458bfc4e72b279a7b1488e2f18f595b772e0` over 1012 complete inputs.
- Complete non-document repair fingerprint: `30b2ad859a0a1f224d5c052693d4c4063d679248d1b8e8c0671fbd7d1d68f753` over 32 complete inputs.

I independently recomputed both values and verified the saved inventory against actual modified and untracked files. Native inputs comprise `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` and every `.rs` under `src`, `tests` and `examples`. Repair inputs comprise modified/untracked non-document files relative to the previous head, including generated fixtures and both new test files. Both recipes sort POSIX relative paths, then SHA-256 the concatenation of `path + NUL + lowercase SHA256(file bytes) + LF`. The individual repair inputs appear below. Documentation and evidence packaging are intentionally outside these source fingerprints.

## Findings addressed and retained boundaries

1. **Attachment timing.** `session/attach` has its own bounded lifecycle lane and no generic Web response deadline. Native's pre-submission initialization and absolute first-submission readiness deadline own that wait. This removes the old 30-second attachment timer without changing actual socket close/error handling, sent-mutation uncertainty, correlation, final Host admission or no-replay semantics. The controlled-clock tests advance 25 or 61 seconds before submission, retain connection/zero-turn facts for another ten seconds, then complete attachment and exactly one authorized turn. A separate test proves real carrier loss still produces one uncertain attachment after an unbounded wait. These are client-boundary tests, not claims of actual elapsed Native deadlines.

2. **One startup acquisition owner.** `prepare_mcp_generation` passes the exact acquired transport to `discover_acquired_catalog`. That helper cannot acquire, replace or spawn a transport. It shares existing discovery coalescing and the generation/epoch-keyed validated catalog. Pre/post discovery transport-health checks, canonical declaration validation, stable epoch and the outer coordinator epoch fence remain. Cancellation during a potentially physical acquire still retires and continues polling that owner; dropping the subsequent listing waiter leaves the registered transport in the generation's retirement path and waits for `retire_and_close`. The actual-stdio tests gate ownership, transport loss, invalidation and close publication, and require one generation, zero/one list, zero tool effects, occupied preparation permit while cleanup is held, proven PID reap or retained unproven-settlement evidence. No second handshake owner exists inside the cancellable listing phase.

3. **Whole new-Conversation submission path.** Session creation remains a side-effecting durable catalog operation and moves into the same finite metadata lane as inert Session/node/cwd reads. It performs no MCP initialization. Attachment waits now have 32 positions, reusing the concrete existing Native external-attachment bound; they cannot consume the six ordinary or two metadata positions needed by independent first submission. Web, TUI, Native associations and Host accounting agree on 32 attachment + 8 observation + 6 ordinary + 2 metadata + 2 control = 50. Host noncontrol admission stops at `max_connections * 48`, leaving two controls per configured connection. Association ownership survives response disposition; the 33rd local attachment is refused before RPC ID/send. Existing bounded pending/uncertainty accounting, final-proof reservations, metadata response timers and no-replay rules remain. Native/client tests inspect exact disposition release, all lanes and physical-owner control reserves. Fresh root replacement and Host-owner invalidation still reject submission/attachment; no authorization cache was added.

4. **Reconnect transport readiness.** Native initialize plus inert catalog synchronization publishes global connected readiness before Session runtime restoration. Each bounded Session actor restores independently, with its existing operation token, generation, wanted intent, exact target, Host admission and observation proof. Its completion cannot establish stale control. The new completed-history/idle-eviction client fixture keeps A cold while B completes independently, allows A's first submission intent without a turn, then requires one exact new-incarnation turn after A's ACK. Navigation, disconnect, authority replacement and late old-carrier replies remain fenced. Uncertain operations and deletion recovery are still actor-owned; reconnect is not automatic mutation replay.

5. **Cross-boundary/browser proof definitions.** I inspected the actual-browser existing/new destination cases under six held real MCP peers, plus same-authority reconnect. Existing destination holds all listing gates and requires the configured terminal outcome; new destination requires a seventh peer, releases only that new peer and requires its exact tool schema in the first provider request while the original six stay held. The reconnect browser case now starts from a fully acknowledged and observed first attachment, disconnects, and awaits actual Native diagnostics with zero loaded/loading/unloading runtimes and external attachments under the existing one-millisecond idle-grace test policy. It reconnects to the same authority, gates the second physical peer, accepts the exact submission intent with zero provider requests, then requires one provider request with full schema, the same Conversation and new runtime/attachment identities. The separate client fixture supplies completed-history coverage. I do not claim execution results from source inspection.

## Generated artifacts and documentation

I inspected the two final generated diffs: `fixtures.json` and `fixtures.ts` change only `in_flight_requests: 18` to `50`. The v57 schema/types and mandatory wire vocabulary are unchanged; no compatibility readers, public modes or new RPCs were introduced. Cargo manifests, lockfile and pinned toolchain are unchanged in this follow-up.

I read the current repository instructions, development/test guides, protocol generation scripts and current CI selection workflow, together with the updated architecture/invariants/protocol, connection/Session lifecycle, auxiliary/file delivery, issue-499 and Jobs/Agents documentation. The active Jobs/Agents paragraph's obsolete sixteen-request policy was corrected to the current fifty-position policy and canonical transport table. Historical evidence remains attributed to its older source rather than represented as current validation. This repair adds no #504 Workspace prewarm, handoff, draft registry or presentation feature.

## Verification limits

I independently ran `git diff --check` (passed), inspected Git identities/inventory and recomputed fingerprints. I ran no Cargo, TUI, Web unit or browser tests in this review; their executions and CI outcomes belong to separate exact-source receipts. I did inspect the new and modified assertions, synchronization cuts, bounded owners and cleanup paths. This is a fresh review of this complete follow-up relative to `5a86a563`, plus relevant unchanged owners, not a certification of every line of PR #503. Any later change to the fingerprinted files requires reinspection; documentation/evidence packaging can be reviewed separately without claiming a different executable source was reviewed.

## Complete repair inputs

| File | SHA-256 |
| --- | --- |
| `protocol/app-server/fixtures.json` | `8d45e375abf4312c7e924b9bca697710e870c6a9939c630fa8684c0988ead17b` |
| `protocol/app-server/fixtures.ts` | `28eb991cdc0d6cc01051effee5e84b5b5ced6a79f26b2e3621866fcbdac2410b` |
| `protocol/app-server/method-policy.ts` | `242ad261a742698bed8f56b976dc7f7390721a91491ffa173904b5ef797681ae` |
| `src/app_server/admission.rs` | `b6b165e7a78be6470e80f7d87b1baf34c6d365faa4918f978f3557c7800324cf` |
| `src/app_server/connection.rs` | `d2526fad55bb2928b3eb20a3656041e81714839609f4e1e75ea774c76f096ad7` |
| `src/app_server/transport/mod.rs` | `f180b87f632cf5b31aaeb0a12e321e14e857b6db306448fd52abf4b1358336a5` |
| `src/capabilities/coordinator.rs` | `9348311880f9f90907734cd14c487d997a38cd3f61741e07df0d18ef19242196` |
| `src/capabilities/coordinator/tests/parallel.rs` | `2b28f24193e1798bafbf5488ea853b08619b6cf784b5ca730cf1f61c1c8e6932` |
| `src/capabilities/coordinator/tests/parallel/acquired_catalog.rs` | `b409617b15dc94ae8f00af655694d7fd8e1dd7bc5fd2ba6cb8607c2b9082ca34` |
| `src/tools/mcp/connection/catalog.rs` | `2f17c6f724b9d43f7769faf165e7cee66610a3165f22248ac0b7193ea4b5f090` |
| `src/tools/mcp/connection/catalog_tests.rs` | `c62530a5b17f056de386a9d9cea2954d321b926360f29b34f9f4ceaa2008a7c4` |
| `tests/scripted/app_server/control_reconnect.rs` | `3de9fb60878f1cb5390eff4604d7fe16a2e65d11077858f996210dbfce9d7cc7` |
| `tests/scripted/app_server/protocol/admission.rs` | `4482c96ee6f3beddb056a33b7443c22494757a705c7fa1921ff55f4dcceb42a3` |
| `tests/scripted/app_server/transports.rs` | `502701da9400e7bb9a5068c307594feaee3647535db493ba51ebce0be7dea3ed` |
| `tui/src/app-server/client.ts` | `6ac21329ee3dfafd024d207750c330a88ce2c79a6e2b4b35365b9dc0874639af` |
| `tui/test/app-server-client.test.ts` | `9fa8bf443f345da955f267c04c6ec9256004a48b057778798c59e8895cd718e1` |
| `web-console/src/client/app-server.ts` | `d51790a9855083411bbcbcc2d2d8b830abee015ee9ca8cdce93cdcb0b25ea55e` |
| `web-console/src/client/request-pipeline.ts` | `888d59952012ac651b0dec852b95ca3f5301f47d87af21b5dcabb071403431d3` |
| `web-console/src/client/session-lifecycle/system.ts` | `f4c457303dd06ff8bcd62eab7feec1a167ef6e5923211b785a5d504a46c468f6` |
| `web-console/test/agent.test.tsx` | `11d2458901bf9c796cae7de59bd0818a06116995742280ade8dc34b1ef8e8913` |
| `web-console/test/artifacts.test.tsx` | `656ccb97470e72479f9fb29cac5c888c0f3951e0c26257c2d6598232b3e9932d` |
| `web-console/test/authority.test.tsx` | `0a61f8b361ccfa386d0bc76d53457d981a58ec5e909b3e0adde66d17b17df2da` |
| `web-console/test/auxiliary.test.ts` | `019eb9c61ba0ad3bbe236e7ee61b943c62f558558f868be836788a2a6f2cc60b` |
| `web-console/test/client.test.ts` | `577a774dbac40c6cb891627d6269f83dc00fe9c7e37b54a065a636cd9d805676` |
| `web-console/test/commands.test.tsx` | `01a1b05433fbe3d50fc43a66c2a3f957ca7f6013ec8a50f6e34c29c17e6aaf42` |
| `web-console/test/context-seat.test.tsx` | `d0ebe574064614541c949094669bf89f324dda19d47a53976ca522e9aaa962a4` |
| `web-console/test/e2e/mcp-startup.spec.ts` | `801ad96c45a62786aff61dc16700b0d1caa209dbdef3e01e1db1c53bdcca552a` |
| `web-console/test/reconnect-startup.test.ts` | `0060d27d1e84e5d979bf482d218500b6a7f327d10898c3a66eb71eec73747bbb` |
| `web-console/test/request-lifetime.test.ts` | `955e6b65ad82a1c21687395587b3236e67dd928b01545be4ecd3b561514310c8` |
| `web-console/test/upload-dispatch.test.ts` | `b688f154ba3d24896c8d45edcfe2dc58982ab83570325bacda6a4203a164d930` |
| `web-console/test/workspace-admission.test.ts` | `10176791d0bbbcb2296571b3fde59570324c104b9680c17f339fface12923dcb` |
| `web-console/test/workspace-host-admission.test.ts` | `04b1e281340a5190e778294823bfcc79ad57da43fc545818ea131efa3458743e` |

## Actual idle-eviction browser follow-up

Reinspected at 2026-10-24T09:20:01.494759+00:00. The only changed repair input is `web-console/test/e2e/mcp-startup.spec.ts`; all other 31 input hashes and the complete Native fingerprint are unchanged. The complete repair fingerprint above and its individual row are refreshed.

The first browser attempt disconnected an unacknowledged Attach, which correctly created uncertainty and suppressed automatic restoration; that fixture could not establish the requested reconnect scenario. The corrected fixture waits for the first `session/subscribe` ACK, applies the existing native User `app_server.idle_grace_ms = 1` policy through reconcile, waits for diagnostics to prove physical runtime eviction, and reconnects with the second tools/list held. It compares both initialized authority IDs, requires no turn/provider request before readiness, then requires exactly one turn/provider request with exact tools and a new incarnation/claim for the same Conversation. No production code, timeout, ignore or uncertainty rule was changed to make the fixture eligible. There is no sleep used as eviction proof.

No confirmed actionable finding in this fixture correction; source approval remains in force for the refreshed fingerprints. Its execution and final full validation remain separate requirements.

## Runtime-incarnation attachment identity correction

Reinspected at 2026-10-24T09:24:18.656373+00:00. Only the browser test input changed; the complete Native fingerprint and the other 31 repair hashes remain unchanged. The repair fingerprint and input table above are refreshed.

`ClientState.next_attachment_seq` starts at zero in each runtime-client host construction; `admit_attachment_identity` increments it to allocate `attachment-{sequence}`. Thus a new runtime incarnation may allocate the same short attachment ID as the retired host. That short ID is not globally unique and its inequality is not a valid eviction assertion. The shared `sameTarget` authority comparison includes Session, Conversation, runtime incarnation and attachment ID.

The browser assertion now requires the same Conversation, different runtime incarnation and different complete target, which establishes new claim authority without imposing a nonexistent global attachment-ID rule. The exact accepted intent, held second tools/list, zero provider/turn before release, and one full-schema provider request afterward are unchanged. No production behavior, timeout or ignore changed. Both earlier failed fixture invocations remain execution evidence rather than being represented as passing. The source comment was also corrected to say that IDs are local to one runtime host and may repeat after eviction. I independently verified that wording against both the live host constructor and identity allocation; this is a comment-only clarification beyond the assertion correction.

No actionable finding in the assertion repair. Source approval remains in force; focused/full executions and CI are separately required.
