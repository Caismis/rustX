# External performance evidence

Detailed performance captures are stored in this [Google Drive folder](https://drive.google.com/drive/folders/1LbNtoe5Kj_fBW7fU2gEg4wYMhC3AtT8A). Reports, readable summaries, source/build identities and compact indexes remain in Git. Building rustX and running its functional tests do not require Drive access.

The [manifest](manifest.json) maps each original repository path to its Drive file, archive member, byte length, SHA-256 and original Git blob ID. It also records the SHA-256 and byte length of every downloadable bundle. Names and original paths inside bundles are preserved. Links in older reports now open the containing Drive bundle; they do not claim that the original capture was remeasured or regenerated.

## Scope

The source is commit `c5617cd939bfaee2bdb777fd32c7ab7456df5908`, including the PR's later fixed-cost, protocol and workspace changes. The 2026-11-12 relocation covers:

- `docs/pi-durable-cost-model/evidence/`;
- `docs/issue-481/evidence/`;
- `docs/issue-480/evidence/`.

There are 173 byte-exact source files in the external inventory. The 121 compressed capture files, totaling 1,447,265,892 bytes, are removed from the current Git tree. Another 52 structured details and indexes are backed up externally and retained in Git; their `retained_in_git` field is true. Small summaries and other evidence outside this inventory remain in place. Runtime code, test fixtures, benchmark implementations and measurement results are unchanged.

Original captures already compressed as xz/gzip are packed into 16 ordinary tar bundles without recompressing or changing the inner archives. One small main-integration archive is stored as a standalone file. This bounds individual downloads to approximately 90 MiB and preserves the inner archive formats and manifests used by existing analysis scripts.

## Download and restore

1. Find the original path in `manifest.json`. The entry identifies the containing bundle and its observed Drive URL. For a standalone entry, the downloaded bytes are already the original file.
2. Download that Drive file. Check the downloaded file's length and SHA-256 against the bundle entry, for example with `sha256sum rustx-pr485-performance-01.tar`.
3. For a tar bundle, extract into a directory outside the source checkout. The tar members start with their original `docs/...` paths:

   ```sh
   mkdir -p /absolute/path/restored-evidence
   tar -xf rustx-pr485-performance-01.tar -C /absolute/path/restored-evidence
   ```

4. Check the original member's SHA-256 and length against its file entry. Existing inner `.tar.xz`, `.gz` and `.zip` files remain unchanged and can be inspected with their original tools/manifests.
5. Historical analysis scripts may expect these exact repository-relative paths. Supply the restored tree as their data root, or restore the needed paths locally before running such a script. The relocated compressed files are ignored by Git so local restoration does not re-add them to a commit.

The relocation was verified by packing and hashing every source member, uploading each bundle, downloading it again through the Drive connector and comparing the complete downloaded SHA-256 and byte count. A failed verification prevents removal of that capture from Git. The manifest records the completed checks; it contains no temporary authenticated download URLs.

## Access and retention

These are ordinary files in the connected user's Drive. No public or organization-wide sharing was enabled by this task. Readers need the folder owner's Drive permission; the repository does not imply that a private Drive link is publicly accessible. Keep the folder and manifest together, and download any needed evidence before changing its retention or permissions.

The repository retains conclusions, adverse results, workload/build identities and the original measurement caveats. A storage relocation is not a new performance result. Pi, pi-rust and DeepSeek were not rebuilt or benchmarked for this change.

## Git space accounting

This is a forward commit. It reduces the current tracked checkout by approximately 1.35 GiB, before the small manifest/documentation additions, and avoids growing subsequent revisions with new raw captures. The original tree contained 1,590,572,540 tracked bytes; the relocated compressed files account for approximately 91% of that total.

Previously committed blobs remain reachable in Git history. A normal deletion commit does not shrink an existing object database or a full-history clone. No history rewrite, force-push, reflog expiration or garbage collection is performed here. Partial clones and new checkouts can benefit from the smaller current tree; shrinking historical clone/pack size requires a separately coordinated history operation.

Future detailed captures should be written outside the repository and uploaded with a verified manifest. Commit only the report and reviewable summary/identity material. `benchmarks/runtime/pack-evidence.py` can still prepare and verify raw archives; its output belongs in external storage.

## Bundle inventory

| Download | Bytes | Source files |
|---|---:|---:|
| [main-integration-20261110--main-integration.tar.xz](https://drive.google.com/file/d/1nUN2v98E3hXDhNlCBmEug41JKAMBbkgj/view?usp=drivesdk) | 284,008 | 1 |
| [rustx-pr485-performance-01.tar](https://drive.google.com/file/d/1Pm-q9oRDza2AWTzQ6wQNJ4dTx7y_ka1l/view?usp=drivesdk) | 94,371,840 | 3 |
| [rustx-pr485-performance-02.tar](https://drive.google.com/file/d/1W-VnbFTp9ZCKsldSXD3zR2TCALXk9S6A/view?usp=drivesdk) | 94,382,080 | 3 |
| [rustx-pr485-performance-03.tar](https://drive.google.com/file/d/1MgL5rbo0YZDxnaVXtPcCkVgTP46V5CRb/view?usp=drivesdk) | 94,371,840 | 4 |
| [rustx-pr485-performance-04.tar](https://drive.google.com/file/d/1NMntyYzFAsy6I2UFmlH5rTeBdxLn_yuj/view?usp=drivesdk) | 94,382,080 | 4 |
| [rustx-pr485-performance-05.tar](https://drive.google.com/file/d/1F7S7MAYf6IHnQYa7UfxalXWth3Ii_mLi/view?usp=drivesdk) | 94,382,080 | 4 |
| [rustx-pr485-performance-06.tar](https://drive.google.com/file/d/1IEpgLHyfDRDPqG9TA4-34NtBoquvv0tH/view?usp=drivesdk) | 94,371,840 | 3 |
| [rustx-pr485-performance-07.tar](https://drive.google.com/file/d/1RGMPOuaQesn4xDdNvkfIHw0IQkj3X8_o/view?usp=drivesdk) | 94,382,080 | 4 |
| [rustx-pr485-performance-08.tar](https://drive.google.com/file/d/1XONxkwvsyZDUbRL19ptTo8ZW3g8gnqTj/view?usp=drivesdk) | 94,382,080 | 5 |
| [rustx-pr485-performance-09.tar](https://drive.google.com/file/d/1h7XPTYks7HMenaDgDRizkVxotm4f3oWF/view?usp=drivesdk) | 94,382,080 | 6 |
| [rustx-pr485-performance-10.tar](https://drive.google.com/file/d/1xjM3VPI3_prsM9sB0-Floz5U2OsmfQHS/view?usp=drivesdk) | 94,371,840 | 5 |
| [rustx-pr485-performance-11.tar](https://drive.google.com/file/d/1MksXuXdUVdzcQZzq5aDG_sAOinvgsxI6/view?usp=drivesdk) | 94,382,080 | 7 |
| [rustx-pr485-performance-12.tar](https://drive.google.com/file/d/1JWzgG7udLGWiwos8NniYJ45-8KVTWZZt/view?usp=drivesdk) | 94,351,360 | 6 |
| [rustx-pr485-performance-13.tar](https://drive.google.com/file/d/14We0ONQjs5J2et8KbZII5OwZLuJVlpdb/view?usp=drivesdk) | 94,382,080 | 8 |
| [rustx-pr485-performance-14.tar](https://drive.google.com/file/d/1UzbGjMyitkCnpcy6gv0cDqgjoMbTpd_z/view?usp=drivesdk) | 94,382,080 | 10 |
| [rustx-pr485-performance-15.tar](https://drive.google.com/file/d/1MwzfQqRWY1d3m69vaDg65BBHHcFXDlQ5/view?usp=drivesdk) | 94,382,080 | 21 |
| [rustx-pr485-performance-16.tar](https://drive.google.com/file/d/1Y5WUZ9UhSMOUsNok3kIQ2XwIGGWLCfMP/view?usp=drivesdk) | 68,976,640 | 79 |
