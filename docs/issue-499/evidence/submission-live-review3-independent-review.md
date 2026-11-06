# Independent review — metadata admission correction for PR #503

**Verdict: approved for the content identified below; no remaining confirmed actionable findings.**

- Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`.
- Branch: `codex/issue-499-cache-aware-mcp`.
- Committed head beneath the reviewed changes: `06c55eb9ce3f9892aa70d92dea7273f2fac3df59`.
- Inspected base: `00978bbe3072b211efe835950f7f99666e1cc02d`.
- Native input fingerprint: `af7492fe25e1894940a5f38fd6126d9adcf35d14b3dd00c02e9f66ed3aed840a` across 1011 files.
- Repair fingerprint: `b0d5e941598624ee794a9fe378150fb6f17d4720c4dd7dc527ebec3e9fab9b83` across the 23 files listed below.

The reviewer did not author the implementation and ran no Cargo, test suite or CI job. Review followed the actual Native, shared method-policy, Web and TUI changes against the committed head, the surrounding admission/response owners, deterministic regression definitions and the final small fixture changes.

## Reviewed boundaries

The new two-position metadata lane contains only the inert native catalog reads `session/read` and `session/settings`. They cannot initiate MCP preparation, and six ordinary cold attachments no longer consume their admission capacity. Native association admission and the shared Web/TUI policy agree on six ordinary, eight observation, two metadata and two control positions. The finite physical-connection envelope is 18; Host aggregate admission remains bounded by configured connections times 18 and noncontrol admission stops at configured connections times 16. The two control positions remain reserved.

The change preserves existing physical ownership boundaries. Native request associations live through response disposition, and blocked writer handoff does not release their positions. Host operation counters retain detached native owners under the existing settlement path. Metadata adds no bypass to those owners. The reviewed regressions cover writer-disposition reuse, Host saturation, connection retirement and delivery cancellation as the eighteenth in-flight request.

The Web pipeline counts both sent metadata and its validation reservations against two positions, retains the existing 64-entry pending bound, and releases only the matching response/retired reservation. Metadata keeps the existing finite 30-second response clock at actual send and normal response-loss disconnection semantics. Ordinary domain waits retain their existing unbounded-response classification. TUI uses the same two finite metadata positions and preserves its existing absence of a generic response timer; read loss remains observation failure, while transmitted mutations remain uncertain. Neither client retries or replays these requests.

Workspace authorization remains fresh. The operation owner still obtains the exact native Session cwd and checks the current Product Host; the request pipeline retains final Host validation adjacent to dispatch, including navigation, native generation and Host authority fences. No authorization/cwd cache or source-authority widening is introduced. Existing configured-root authorization on registration removal is unchanged. Root disappearance, changed canonical resolution and Host replacement still refuse the submission intent while six attachments remain held.

The changed ordinary-capacity fixture fillers now use methods that actually remain in the ordinary lane. Their original stale-authority, queue ownership, upload and cancellation assertions are preserved. New regressions address known and unknown selected nodes, metadata queueing and response deadlines, slot exhaustion/reuse, and an actual six-Conversation MCP fixture whose discovery gates remain unreleased while only the submitted Conversation reaches its configured deadline.

The final Rust lint cleanup combines the two identical capacity match arms as `Metadata | Control => 2`; it does not merge the lanes or their accounting. The generated `fixtures.json` and `fixtures.ts` changes only update the diagnostic `in_flight_requests` value from 16 to 18. Wire unions, serialized fields and negotiated vocabulary are unchanged, so this scheduling/capacity correction requires no new protocol version. The final browser preference seed uses `new URL(fixture.endpoint).href`, matching the product's canonical persisted endpoint; it changes no timeout, capacity or assertion. The initial noncanonical seed was test setup evidence, not a production exception.

The native submission timestamp/deadline owner, live MCP validation, frozen Attempt snapshots, authored-configuration isolation and dispatch fences remain under their previously reviewed owners. This correction introduces no issue #504 prewarming, new RPC, connection-transfer mechanism or UI presentation.

## Content identity

Fingerprint recipe: sort relative POSIX path strings; concatenate each path, NUL, the file-content SHA-256 as ASCII hexadecimal, and LF; SHA-256 the concatenation. The native set is Cargo.toml, Cargo.lock, rust-toolchain.toml and every *.rs beneath src/tests/examples. Every manifest entry and both composite fingerprints were independently recomputed. The repair set is exactly:

| File | SHA-256 |
| --- | --- |
| `protocol/app-server/fixtures.json` | `9daf61df664bc89b0e1d8fb20d6597da11451143331c27682cf39035b4a42683` |
| `protocol/app-server/fixtures.ts` | `e746c54444c3c41aa9100d48909699d8bbd0857731a37f454f02303c4466ad74` |
| `protocol/app-server/method-policy.ts` | `997fd4fe1587257108067b11fbfe9c21319a642fb922c61056985a7b1d6ed927` |
| `src/app_server/admission.rs` | `494a3253af850e162efc64e9efd37032e3edbf232c8e633cf9d52720e34709a9` |
| `src/app_server/transport/mod.rs` | `1e7616a0c21cfb419bff6343b46717ae563a4a62014352d23f4f28cb8f6288e1` |
| `test-support/fake-provider/src/fake_provider/scenarios/tui.py` | `67839d0bbb6ecacd48ff1bfe54570281aecd3510c9a10c70dad09385ec72eb15` |
| `tests/scripted/app_server/control_reconnect.rs` | `ed7ee1753499b84324b18728d46ce03194a7eae40e7c2cb8c879ede8624cefec` |
| `tests/scripted/app_server/protocol/admission.rs` | `04ffad1dda885a842e78653d14d3c1765cc3304dfe6bb6caf64a780d5cac4014` |
| `tests/scripted/app_server/protocol/delivery.rs` | `bab43b609f2dbe755ca31b9a9188c61725a1bdceb5585d74a6a939468960c7dc` |
| `tests/scripted/app_server/transports.rs` | `69f28f7b0103eaa0fde6b25b1fbb97b2f13a00dcee6af6bc9ccf7a99a972c0f9` |
| `tui/src/app-server/client.ts` | `01db40597d56b51865cb93537f3192fee9acecd43ed7d6873948913a612d58c2` |
| `tui/test/app-server-client.test.ts` | `ae1cb1c57db893b9918e207e9df85bd62dca3a314b7466e6cce8e67133612eba` |
| `web-console/src/client/app-server.ts` | `a3dacd4a61327af17042b4c89d0293b982ecd81e19c3ef90f48a79ebc4711c08` |
| `web-console/src/client/request-pipeline.ts` | `e67d34f43d3f8116a71631e2b6405560e45b897db6c95d683e46c708677218b5` |
| `web-console/src/workspaces/session-admission.ts` | `0663efbd247c5cb2a16bf2c498ee9620d2aa9c5e62d5fc57fd98e351a9a18dbe` |
| `web-console/test/attachment-node.test.ts` | `4ae408f21ad9fb96affef923a417dbfe6c459194c75bc9f99a71f5c48dc442ed` |
| `web-console/test/client.test.ts` | `328504c67ba55c464669747c00ddd983d89bef840f0a438fe0f12e2ecd299b56` |
| `web-console/test/dispatch-linearization.test.ts` | `b8aa081bdeb5da9cb608897d55a1b89f47ac98d7d6f2a61be9467fa1ff301384` |
| `web-console/test/e2e/mcp-startup.spec.ts` | `90b393271e932b0b92d0329ecc24248213527bd1bc112e1c8b039d30bebc593b` |
| `web-console/test/observation-authority.test.ts` | `230d61af7bbb41c128fd6b4e602f5e162f8e070de5f0f054ca7463fef8419ba2` |
| `web-console/test/request-lifetime.test.ts` | `559cebfe881afd62117a58863103e324abae09b8c600a1ba2ee67b2ce37c547e` |
| `web-console/test/upload-dispatch.test.ts` | `32bc56a529f8197ff4dcbaecccf274a6942ec3e7b412a994205e1346d9210d4a` |
| `web-console/test/workspace-host-admission.test.ts` | `4a42d99b0eb29bf4590b4f2cbea595a9b84a0cbb88aa092ea74c1df340da1278` |

## Validation limits

This is independent source review only. Root-owned focused/full commands, actual browser execution, measurements and CI have separate receipts; this review does not turn a pending or historical check into a pass. Complete local validation, FULL CI and current Merge verification must match the resulting final head/base/merge commit. Subsequent substantive source changes require renewed review. This approval does not authorize merging or auto-merge.
