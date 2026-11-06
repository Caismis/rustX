# Independent review receipt — PR #503

**Verdict: approved for the reviewed source; no remaining confirmed actionable finding.**

## Reviewed identity

- Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`
- Branch: `codex/issue-499-cache-aware-mcp`
- Committed HEAD beneath the reviewed working changes: `385fd43da3ea820bcc3b7768e4550f070209c12d`
- Inspected base: `origin/main` at `00978bbe3072b211efe835950f7f99666e1cc02d`
- Final native source fingerprint: `64e9a3768527922cae763bbade69b7774602097f75fdbd1c7207d9dbada95b46` across 1010 files.

Fingerprint recipe: include `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, and all `*.rs` files beneath `src`, `tests`, and `examples`; sort relative POSIX path strings; SHA-256 the concatenation of each path, NUL, its file-content SHA-256 hexadecimal string, and LF. This identifies the reviewed dirty source independently of the later commit identity.

## Scope and resolved findings

The reviewer did not author the implementation. Review used the actual source and deterministic regression definitions, repository instructions, and the final owner clarification. It covered native receipt clocks and ownership, initial live discovery and queue bounds, cancellation and physical settlement, client submission/attachment paths, configuration resolution, runtime publication, and immutable Attempt admission.

Earlier confirmed findings were fixed and re-reviewed: loading cancellation and activation handoff; retained exact-node submission receipts before metadata waits; drain and stale-owner resurrection fences; durable recovery intent timing; blocking publication behind background ownership; retention of candidates at a busy native frontier; newer catalog observations after a retained candidate; recovery across sibling runtime publication and unrelated authored adoption; continuous per-source authority against binding ABA; stale success/error suppression; and failure-diagnostic chronology. The final review also checked pending authored model/context/source intent isolation, schema/epoch dispatch fences, no-op publication, queued-candidate shutdown, and the explicit exclusion of issue #504 prewarming.

New regression definitions establish the relevant synchronization cuts with gates/watches and assert provider/list counts and tool inventories. The existing separate 60-second frozen-child startup supervision remains strict and is documented separately from the configurable Root/enabled-Workflow submission budget.

## Limits

This receipt records independent source review, not test execution. The reviewer ran no Cargo build, test suite, or CI job, and does not claim validation from earlier commits applies to this source. Final local verification, FULL CI, and Merge verification for the actual final head/base/merge commit remain separate requirements. Approval does not authorize merging or auto-merge. Any subsequent substantive source change requires renewed review.

## Follow-up review: TUI CI prerequisite and owned gate cleanup

The subsequent five-file repair was independently reviewed and approved, with no remaining actionable finding. It builds all native integration binaries in the TUI lane, checks the required sibling supervisor before allocating test resources, races discovery against actual attachment outcomes, unwinds waits on watchdog cancellation, and closes the gate before joining native shutdown with nested cleanup. Three deterministic fixture regressions cover early attachment outcomes, cancellation, and explicit close. This is a test/workflow/documentation change; production Rust and TypeScript remain unchanged. The native fingerprint above still matches. No tests were executed by this reviewer.

The reviewed five-file working diff is relative to head `e8a446fd37dfa469df4b5553f00d1581fccc2cdd`. Its sorted-path content fingerprint (same path/NUL/file-hash/LF recipe) is `527052a3936e91c3e35310590ffa72bb7828e213d02f8454907063b270b8b5fd`. The final follow-up also corrects the existing missing-binary build hint to `cargo build --bins`; that text-only change is approved.

Reviewed file-content SHA-256 values:

- `.github/workflows/ci.yml`: `79ebbe057f34330a8615cb2d73369bb2e89369563edc4673f65e608160ce51b2`
- `tests/README.md`: `0b657c587dcece58b190750c707cb76214a718664a98662325a3b8742a467d9b`
- `tui/test/integration.test.ts`: `293f0064ef3f32eb820789c9772c0803afb5cb7a81942c3cf8a4057df58f2058`
- `tui/test/support/mcp-startup-gate.ts`: `78ae6d1967502b2e690770d877f4a69548aa7302f841c443f7f61f6d58ca88a8`
- `tui/test/mcp-startup-gate.test.ts`: `eaec6d351d0b50787c3c219e7c10a268fb045e594565e26ebe1f1a226cdbcfcf`
