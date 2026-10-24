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
