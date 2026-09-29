# Issue #398 — cap-std upload pilot (historical decision)

## Decision: No-Go

Session uploads keep the native `std + nix` implementation. The cap-std pilot was
a test-only experiment; #426 retired it, so no candidate code, comparison test or
cap-std dependency remains in the repository.

## What was evaluated

cap-std/cap-fs-ext 4.0.3 capability directories replaced the mkdir/open/write
mechanics of upload materialization in a `#[cfg(test)]` candidate that composed
the native claim, publication and commit owners. Deterministic probes compared it
with the native owner for path policy, symlink substitution, exclusive creation,
modes and leaf types, sync and publication barriers, directory locks, lifecycle
admission and operation cost (the FS-01–FS-12 matrix of the historical record).

## Why it was rejected

- Containment is weaker than rustX's full-path no-symlink policy. cap-std follows
  symlinks that stay inside the capability root, final-only no-follow still
  accepts a symlinked ancestor, and internal `..` is permitted. The native
  component walk, basename validation, publication re-open with dev/ino identity
  and regular-file checks therefore all had to stay.
- Linux `O_PATH` directory handles can neither sync nor `flock`, so the candidate
  had to reopen readable descriptors. `Dir::try_clone` shares one open-file
  description, so clones are not independent lock owners.
- Holding a capability is not lifecycle admission; native Conversation access,
  exclusion and ownership locks remained the authority.
- The pilot added 2 direct and 9 transitive package/version pairs, and removed
  no native concept. Measurements showed no performance advantage. Complexity,
  not speed, decided No-Go.

## Authoritative implementation

`src/local_runtime/session/uploads.rs` owns upload policy and materialization:
`validate_name`, `validate_workspace`, `stable_directory`, `directory_at`,
`session_directory` and `UploadRegistry::{claim, materialize,
verify_materialized, receipt_ref, receipts}`, published through
`SessionCatalog::commit_uploads` by `SessionController::upload`. Its regressions,
including the native sync-failure, ready-commit-failure and declared-path
substitution cases that the pilot first exercised, live in
`src/local_runtime/session/uploads/tests.rs`. Directory lock semantics are
tested in `src/runtime/local_storage.rs`.

## Historical experiment

The complete, runnable experiment is at merge commit
`8b59e770225cf8ae39bfdfe2a6e50b0137bfa146` (PR #408). It contains the
candidate module `src/local_runtime/session/uploads/cap_std_validation.rs`, the
`Cargo.toml` dev-dependencies and matching `Cargo.lock`, and the full
decision record with the FS matrix, dependency audit, measurements and
reproduction commands in this file. The PR head `4b4a52de` has identical
content; `7880d1b5` was only the audit base and predates the experiment.

```sh
git show 8b59e770:docs/issue-398-cap-std-validation.md
git worktree add --detach ../rustX-398 8b59e770225cf8ae39bfdfe2a6e50b0137bfa146
cd ../rustX-398
cargo test --lib --all-features --locked cap_std_validation -- --nocapture
```

Reconsider only with a separately bounded proposal that removes the native
full-path traversal and interop complexity while preserving the native owners.
