# Production geometry benchmark

This opt-in lane builds the real geometry/foundation browser fixtures as production bundles. It uses the same explicit `esnext` target as the original temporary baseline and candidate measurements; the ordinary application build uses Vite's default target. This fixture lane therefore does not claim identical application build settings. It leaves the default pinned screenshot lane unchanged. Install the lockfile dependencies first and use the same pinned Playwright 1.63.0 browser image documented in `web-console/scripts/browser-tests.sh`.

From the repository root, with a running pinned browser server:

```sh
RUSTX_BROWSER_WS_ENDPOINT=ws://127.0.0.1:5277/ \
  bash web-console/scripts/geometry-performance.sh
```

The script builds `web-console/dist/geometry-performance`, copies the immutable CAD/GLB/STL fixture files into that output, serves it on localhost:5174, and runs three independent fresh browser contexts per workload. Set `RUSTX_E2E_FIXTURE_PORT` to change the local server port. Run without concurrent builds or tests and retain the emitted browser/renderer information; hardware, CPU quota, browser transport, and cold code-cache conditions affect results.

Raw sample arrays and worker profiles are under `web-console/test-results/geometry-performance`; the Playwright JSON report is `web-console/test-results/geometry-performance-report.json`. The latency test measures trusted browser input through actual model submission, GPU fence completion, and the next animation-frame rendering opportunity. It does not observe compositor or physical display presentation. Acceptance requires a usable approximately 200-MiB model within 200 ms and every observed interaction below 50 ms. P95 remains diagnostic and cannot replace the maximum. MSAA, workload sizes and admission limits stay fixed; actual moving/complete geometry and drawing-buffer sizes are recorded independently.

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

To include the STL lazy-loading regression, first serve the ordinary production
application with `pnpm preview --port 5173 --strictPort` in another terminal,
then run all three request-boundary cases through the production fixtures:

```sh
RUSTX_BROWSER_WS_ENDPOINT=ws://127.0.0.1:5277/ \
  bash web-console/scripts/geometry-performance.sh geometry-code.spec.ts geometry.spec.ts \
  --repeat-each 1 \
  --grep 'cancelled CAD code wait|two actual split CAD owners|production startup defers geometry'
```

Set `RUSTX_E2E_PREVIEW_PORT` when the application uses another port. Vite's
development URL-import modules are metadata; request accounting distinguishes
them from actual WASM bytes and executable CAD glue/Worker code in both lanes.
