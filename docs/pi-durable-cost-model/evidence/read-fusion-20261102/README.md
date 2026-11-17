# Read-fusion validation evidence

Source baseline: `84422d2bf11550fb1ba31bf1023903b4737a4360`.

`checks.tar.xz` contains 42 files: the source patch, before/candidate source hashes, unchanged Cargo hashes, test build command/output, all 15 check commands/logs, runner, accepted summary, and a source-linked recalculation of prior WAL ownership shares. It contains no executables or new runtime performance products.

- 2,359 distinct tests passed; zero failed; one existing ignored.
- Fmt and release/all-target/all-feature Clippy with deny-warnings passed.
- Correctness build: release dependencies, root package O0, all features. These are not production performance measurements.
- SHA256: `00077001b78e52e7e817bde8e91431db004a5f502a55f88b9b9dc95ed6377b6c`; compressed bytes: 121,892.
- `manifest.json` records every member length/hash; the newly generated archive was read back and every member checked without extraction.

`prior-write-inspection.json` only recalculates the unchanged prior content-round archive (`0d5aa082ee331274948a0946cb31188e593e9b0d5a08850151861d036914f741`). Endpoint page ownership does not identify every frame's write-time cause and is not a removable-cost percentage.

No new whole-request latency, CPU, process-write, WAL or Pi/DeepSeek measurements were made. The prior formal results do not measure the new source.
