# Independent review — second functional repair for PR #503

**Verdict: approved for the source identified below; no remaining confirmed actionable finding.**

- Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`.
- Branch: `codex/issue-499-cache-aware-mcp`.
- Committed head beneath the reviewed changes: `f7cae82a07e3c1fa8263aad596da157b5669ee4a`.
- Inspected base: `00978bbe3072b211efe835950f7f99666e1cc02d`.
- Native input fingerprint: `52692c18a189f515b3c86f22f614834fd46d92d576d0f36c898a3a32932d6aef` across 1011 files.
- Eleven-file repair fingerprint: `55236d274ba140260c701b56f63cc3302b8733c740a27f8852ee25bd30708b1f`.

The reviewer did not author these changes. Review followed the three changed production paths and their deterministic regression definitions, including the surrounding source authority, immutable Attempt, pending authored configuration, transport acquisition and external-effect owners.

The Web submission receipt now obtains the existing Session operation admission and passes its object proof into the request pipeline. This covers both new and existing Sessions, uses the fixed native cwd, and performs the established final Product Host validation in the reserved dispatch slot adjacent to send. A known create acknowledgement remains captured if the later receipt is refused. Registration removal continues to preserve configured-root authorization, as required by the already-merged Host contract; root disappearance and changed canonical resolution are the appropriate rejection cases.

Runtime refresh rebasing begins with the currently adopted complete base registry and overlays the adopted ordinary/live catalog. Root Agent controls therefore survive without a Workflow recomposition, while selection still excludes profile-composed dispatchers from the ordinary selectable catalog. This uses adopted inputs, preserves pending authored intent, and leaves active Attempt snapshots frozen. The new no-op and held two-step Attempt regressions assert control inventories and exact provider/list/call counts.

Invocation validation retains the exact discovered transport, invalidation epoch and complete canonical declarations. It no longer acquires a second transport after discovery. Loss of that transport fails the current invocation before tools/call; only a later invocation may reconnect. The existing full-schema comparison, cancellation checkpoint, generation ownership and locked epoch/enqueue fence remain in place. The new regression proves one handshake/list and zero calls for the failed invocation, followed by one replacement handshake/list and one call for the next invocation.

The reviewed browser regressions also pass the required cancellation signal, use bounded receipt observation and own gate/helper cleanup. This repair adds no issue #504 prewarm feature or public policy mode. The final selector-only correction scopes the expected submission-failure alert to the exact Chat tabpanel, avoiding strict-locator ambiguity with another truthful UI alert. The required authorization-error text and all zero-intent, zero-attachment, zero-loading/runtime/list/provider assertions remain unchanged. Reverting only this selector reproduces the preceding reviewed browser fixture hash.

The additional TUI regression cleanup change registers the same idempotent owned cleanup with `t.after` immediately after provider acquisition and still awaits it in `finally`. The shared promise prevents duplicate shutdown; nested `finally` blocks attempt gate closure before native shutdown, then fixture removal and provider settlement even if an earlier cleanup fails. The newly added failure diagnostic preserves native stderr and provider evidence. Other TUI cases are unchanged, and the final native fingerprint is unchanged by this test-only delta.

The follow-up emulator inventory assertion explicitly includes `mcp_submission_refused` among the named zero-request scenarios and requires its steps to be empty; all other registered scenarios retain the nonempty-step assertion. This matches the refused-submission browser proof and does not weaken provider request matching. A final lint-only cleanup spells the inferred `Default::default()` as `toml::Table::default()` in the new profile-control test helper. The initializer is equivalent: `toml::Value::Table` already requires that table type. Reverting only that spelling reproduces the previously reviewed file hash; all other nine file contents remain unchanged. The final fingerprints above include this cleanup. Production behavior is unchanged by the lint repair.

Fingerprint recipe: sort relative POSIX path strings; concatenate each path, NUL, the file-content SHA-256 as ASCII hexadecimal, and LF; SHA-256 the concatenation. The native set is Cargo.toml, Cargo.lock, rust-toolchain.toml and every *.rs beneath src/tests/examples. The repair set is exactly the following eleven files:

| File | SHA-256 |
| --- | --- |
| `src/capabilities/coordinator/readiness.rs` | `3c395b0992fba1d1c4265883ab6adaf77ccdb58332fb87f5a2b1f8137b104b65` |
| `src/tools/mcp/connection/catalog.rs` | `d2fe92866e9d99a877269aa49341cdb832f358c6e9d6b4b7fc2f6ddd7315a28a` |
| `src/tools/mcp/connection/catalog_tests.rs` | `96464e298072b3381436c2242bed50543561af992684e3cfc6bd039179d5a087` |
| `test-support/fake-provider/src/fake_provider/scenarios/tui.py` | `277967d9fa4997812ecd2ee42aaa6120f5169f849c9b182da7417ad350379182` |
| `test-support/fake-provider/tests/test_scenario.py` | `e0aee81ddd65759ec0abeb01efa66544c45d204cc9989a28b4bc628927f30d9f` |
| `tests/scripted/app_server/configuration/mcp_readiness.rs` | `af98156a76c97b2ad666e64196a8dce9326dfc3dbbde5dc1bba035370cfc6b55` |
| `tests/scripted/app_server/configuration/mcp_readiness/profile_controls.rs` | `925c63681cf06fe976f43e18c0fc08da6cfefd5b79264eabcec4c61c8e6dcdf3` |
| `tui/test/integration.test.ts` | `2c255786e69d57c7d89215010806a15ee62e35d0ca70553914419c818dca2a2a` |
| `web-console/src/app/new-conversation/port.ts` | `82a4f7879a0099c397a710df8584f809648cc88282a5222c25d5d0c132283878` |
| `web-console/test/e2e/mcp-startup.spec.ts` | `1a421eb61331c98af89adafc445ba12585b33ac04dd20f14dc492fca4252966c` |
| `web-console/test/workspace-host-admission.test.ts` | `94fe9da13816c3d7ee0b8c07e645fce952e3b7a832298d8af9f710747c4c6757` |

This receipt is independent source review only. The reviewer ran no Cargo, test suite or CI job and makes no fresh validation claim from historical green results. Full local verification and FULL CI/Merge verification for the actual resulting head/base/merge commit remain separate requirements. Subsequent substantive source changes require renewed review. This approval does not authorize merge or auto-merge.
