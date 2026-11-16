#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${RUSTX_BROWSER_WS_ENDPOINT:?Set the pinned Playwright browser server endpoint first.}"
node node_modules/vite/bin/vite.js build --config test/geometry-performance.vite.config.ts
cp -R test/fixtures/geometry dist/geometry-performance/test/fixtures/
if [[ -n ${RUSTX_GEOMETRY_LARGE_STEP_PATH:-} ]]; then
  cp -- "$RUSTX_GEOMETRY_LARGE_STEP_PATH" dist/geometry-performance/test/fixtures/large-step.step
fi
if (( $# == 0 )); then
  set -- geometry-latency.spec.ts
  if [[ -n ${RUSTX_GEOMETRY_LARGE_STEP_PATH:-} ]]; then set -- "$@" geometry-step-latency.spec.ts; fi
fi
node node_modules/@playwright/test/cli.js test --config playwright.geometry-performance.config.ts "$@"
