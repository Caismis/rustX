# Production geometry benchmark

This opt-in lane builds the real geometry/foundation browser fixtures as production bundles. It uses the same explicit `esnext` target as the original temporary baseline and candidate measurements; the ordinary application build uses Vite's default target. This fixture lane therefore does not claim identical application build settings. It leaves the default pinned screenshot lane unchanged. Install the lockfile dependencies first and use the same pinned Playwright 1.63.0 browser image documented in `web-console/scripts/browser-tests.sh`.

From the repository root, with a running pinned browser server:

```sh
RUSTX_BROWSER_WS_ENDPOINT=ws://127.0.0.1:5277/ \
  bash web-console/scripts/geometry-performance.sh
```

The script builds `web-console/dist/geometry-performance`, copies the immutable CAD/GLB/STL fixture files into that output, serves it on localhost:5174, and runs three independent fresh browser contexts per workload. Set `RUSTX_E2E_FIXTURE_PORT` to change the local server port. Run without concurrent builds or tests and retain the emitted browser/renderer information; hardware, CPU quota, browser transport, and cold code-cache conditions affect results.

Raw sample arrays and worker profiles are under `web-console/test-results/geometry-performance`; the Playwright JSON report is `web-console/test-results/geometry-performance-report.json`. The existing latency test measures trusted browser input through actual model submission, GPU fence completion, and the next animation-frame rendering opportunity. It does not observe compositor or physical display presentation. Antialiasing, default outlines, workload sizes, admission limits, and the 200 ms cold / 50 ms interaction P95 targets are unchanged.

Evidence runs write all results before enforcing targets. To run the explicit acceptance gate:

```sh
RUSTX_BROWSER_WS_ENDPOINT=ws://127.0.0.1:5277/ RUSTX_GEOMETRY_BUDGET=1 \
  bash web-console/scripts/geometry-performance.sh
```

To retain the separate whole-gesture, same-copy remount, picking, two-pane, and retirement observations:

```sh
RUSTX_BROWSER_WS_ENDPOINT=ws://127.0.0.1:5277/ \
  bash web-console/scripts/geometry-performance.sh geometry-performance.spec.ts
```

Those observations use their original harness and are not interchangeable with the event-to-GPU latency endpoint. Ordinary evidence completion does not mean the performance targets passed; report the explicit gate separately.

The real-browser code-sharing regressions hold the actual WASM fetch while cancelling an owner, and reopen two genuine split CAD owners with fresh Workers and one code fetch:

```sh
RUSTX_BROWSER_WS_ENDPOINT=ws://127.0.0.1:5277/ \
  bash web-console/scripts/geometry-performance.sh geometry-code.spec.ts --repeat-each 1
```

Their attachments include observed compile/reuse wait times and complete resource retirement. These lifecycle observations include outline completion and any retained shader state; they are separate from cold acceptance.
