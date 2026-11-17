# PR #485 history cleanup

Published on 2026-11-16 after explicit approval. Only `refs/heads/codex/pi-durable-cost-model` on `https://github.com/Caismis/rustX.git` was replaced with `--force-with-lease`, from `c7b2338ae80f34a7e042da17cd0ca9031bc09b8d` to `bef6c1963fc8a6301c1c3a1bf789bc681e72df7f`. The full tree of that update stayed `669763929f57155eb32ded4c379dda70e4602708`. This was not a repository-wide rewrite, branch deletion, or merge. It does not report CI or pull-request merge completion. Main has since advanced and is being integrated separately; that work is outside these measurements.

The 2026-11-12 relocation remains a forward deletion. [manifest.json](manifest.json) is not part of this rewrite and must stay byte-for-byte immutable. Its `git_history` value, `Forward deletion only. Existing historical objects remain reachable; no history rewrite or force-push.`, describes that relocation only. See [README.md](README.md).

## Measured scope

The pack comparison used a frozen snapshot of 224 public branch tips. The other 223 refs, including main `c076354d647c1a1832e9af5e005696ecb5579285`, kept their commit IDs. No branch was deleted. git-filter-repo 2.47.0 removed compressed captures under `docs/pi-durable-cost-model/evidence/` from the frozen PR-only range. The allow-list was 103 paths (101 `.xz`, 2 `.gz`). Captures already on main, and captures on branches that do not contain this PR, stayed.

Capture-blob attribution, which is not the pack delta: `main` 25 blobs / 125,638,032 bytes; `pr` 124 / 1,481,311,282; `remote_with_no_pr` 72 / 208,151,876. After filtering, 56 archival blobs (1,306,911,858 logical bytes) were no longer reachable from the frozen public heads.

Pack-related bytes were measured on two standalone bare mirrors of those 224 tips, with no alternates or shared object hardlinks. Both were repacked with four threads, `windowMemory` 128 MiB, and normal compression. Totals include pack, index, bitmap, and reverse-index.

| Frozen snapshot | Pack-related bytes | MiB |
|---|---:|---:|
| Before | 1,708,059,235 | 1,628.932 |
| PR-only cleaned | 404,786,746 | 386.035 |

MiB is bytes / 1024^2, rounded to three decimals. Reduction: 1,303,272,489 bytes, 76.301% (recorded 76.30136369363092%). These are object-store figures for that frozen snapshot, not a runtime benchmark, clone timing, the size after main's later advance, or current GitHub server size. The publication does not prune other clones. Old diagnostic branches, worktrees, reflogs, forks, and historical pull-request refs can still hold the removed objects. Non-sensitive cleanup does not qualify for GitHub's sensitive-data purge: [Removing sensitive data from a repository](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository).

## Identity

`bef6c1963fc8a6301c1c3a1bf789bc681e72df7f` is the published rewrite tip, not a permanent branch head. Later commits, including the separate main integration, can advance it. The relocation source recorded in `manifest.json` is `c5617cd939bfaee2bdb777fd32c7ab7456df5908`, mapped to `d52fc8b61b1b7180c54c52afc7e735942391310c`.

[commit-map.txt](commit-map.txt) has 359 old/new pairs. Three IDs did not change: `989d74d12d211a38eee5ad1f07ec18c0251a0de5`, `9a50d5ce6d51fd53aa6f20111ccf78daa2a424d7`, and `f9d2b5be6f61711df94499aaaa118a68246ffe0b`. The map identifies already-rewritten PR commits. It is not a list of commits to cherry-pick again, and local unpublished commits are not in it.

Messages, author and committer identities and times, encoding, and mapped parent order were preserved. No original PR commit was GPG-signed. Artifact-only commits remain, including those that become empty. Source, tests, configuration, protocol, and readable reports were preserved.

## Backups

Current-tree bundles remain in the [Drive folder](https://drive.google.com/drive/folders/1LbNtoe5Kj_fBW7fU2gEg4wYMhC3AtT8A). Their URLs and lengths are the inventory in [README.md](README.md): 121 compressed captures, 1,447,265,892 bytes, plus 52 structured files retained in Git.

Eight historical versions missing from that manifest, including local diagnostic history, total 74,550,646 member bytes. They were packed, uploaded to the same folder, downloaded, and SHA-256 verified. The exact member list is [historical-captures-manifest.json](historical-captures-manifest.json). Do not fold it into `manifest.json`.

| | |
|---|---|
| Archive | [rustx-historical-performance-captures-20261112.tar](https://drive.google.com/file/d/1sM04BJdfryQ6zdXBepp5zbBwHg2e0pfu/view?usp=drivesdk) |
| Bytes / SHA-256 | 74,567,680 / `4687c8c13b2f8697b4d93a9e358a0422aa996bc485ad7fdc749265918f79800c` |
| Supplement manifest | [Drive copy](https://drive.google.com/file/d/1F3FffJSWZ5IXmEQQt2AEKbjhjiy9yZpM/view?usp=drivesdk) |

Archive members use `objects/<git-blob-id>/<original-path>`. Four paths are under `docs/pi-durable-cost-model/evidence/` and were on the removal allow-list. The `docs/issue-481` and `docs/issue-480` versions are older blobs; this rewrite did not retarget those branches.

Pre-publication validation archive, not main-integration evidence: [history-cleanup-validation.zip](https://drive.google.com/file/d/1G1ypHd3Y13cu5x9uKCxOREZNiqmVeWVo/view?usp=drivesdk), 32,663 bytes, SHA-256 `e046cd058d5b6534dbaa453976ecf8befd1c5b8c3931bb3820d5ba2753a0daeb`. It contains the frozen ref snapshot, validation, maps, and pack measurements.

## Migration

Fetch the current `codex/pi-durable-cost-model` and use that tip. Confirm `bef6c1963fc8a6301c1c3a1bf789bc681e72df7f` is an ancestor of it. Do not require the tip to equal that commit, and do not move onto it once the branch has advanced. Preserve local commits and dirty work. Never force-reset a dirty worktree.

Never merge old PR ancestry, including `c7b2338ae80f34a7e042da17cd0ca9031bc09b8d` or any pre-rewrite parent, into the cleaned branch. A merge brings the removed history back. Cherry-pick and rebase apply patches; they do not import old ancestry by themselves. Replay only reviewed local patches onto the current cleaned head. Only a patch that adds an archived capture recreates that blob, so drop those paths instead of taking the old tree. Do not cherry-pick already-mapped PR commits again. Leave old diagnostic branches and worktrees untouched.

## Validation

Checked before publication:

- 359 original PR commits remain, including artifact-only commits that become empty.
- Messages, author and committer identities and times, encoding, and mapped parent order were preserved. Original signed commits: 0.
- 25,896 per-commit path differences were deletions of allowed compressed capture paths only.
- Full tree identical: `669763929f57155eb32ded4c379dda70e4602708`. The other 223 snapshot refs were unchanged.
- A fresh independent clone that did not reuse local objects was verified, and `git fsck --full --strict --no-reflogs` passed, with no permanent alternates.
- The eight-member historical archive was downloaded and matched the recorded length and SHA-256.
- No Pi, pi-rust, or DeepSeek run, and no new performance claim.

After publication, the only check was an exact remote ref readback of the lease update. No new clone verification or strict fsck was run afterward.
