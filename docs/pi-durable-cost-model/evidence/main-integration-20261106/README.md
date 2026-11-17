# Main integration validation, 2026-11-06

See [the integration report](../../2026-11-06-main-integration.md).

- `validation.json`: parents, format versions and final results.
- `inputs.json`: SHA-256 of final tracked source/test/configuration inputs; the merge commit identifies the complete tree, including unchanged reference images.
- `checks.tar.gz`: complete final logs, four successful browser JSON reports, generated-protocol drift proof, and the local prepared-addon startup configuration.
- `checks.manifest.json`: unpacked member lengths and hashes. The archive hash is in `validation.json`.

The external combined command initially failed only the old documentation version text. The archive retains that failure and the successful 31-test contracts rerun; the other six suites passed. Ignored tests were not forced on. The lifecycle before/final logs preserve the two deterministic failures before the acknowledged-Attach fix and the successful combined regression run.

The four final browser reports must contain 639 distinct spec IDs, all expected passes, with no retries/flakes, skips or runner errors. References were not updated. The local startup override reuses prebuilt native addons to avoid concurrent node-gyp rebuilds in one checkout; assertions, pixel comparison, pinned browser image and one worker per shard are unchanged. A temporary port collision or failed preparatory launch is not a browser test result.

These are correctness checks, including existing fixed workload/latency-diagnostic browser tests. They are not new timing evidence or a rerun of Pi/DeepSeek. macOS/other platform and remote merge verification remain separate CI gates.
