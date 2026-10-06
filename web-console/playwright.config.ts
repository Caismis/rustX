import { defineConfig } from '@playwright/test';
const previewPort = Number(process.env.RUSTX_E2E_PREVIEW_PORT ?? 5173);
const fixturePort = Number(process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
const previewURL = `http://127.0.0.1:${previewPort}`;
const fixtureURL = `http://127.0.0.1:${fixturePort}`;
const wsEndpoint = process.env.RUSTX_BROWSER_WS_ENDPOINT;
if (!wsEndpoint) throw new Error('Use pnpm test:e2e or pnpm test:e2e:update: references require the pinned browser container.');
export default defineConfig({
  testDir: './test/e2e', testMatch: '*.spec.ts', workers: 1, timeout: 120_000,
  updateSnapshots: 'none',
  // Every browser reference goes through expectStableScreenshot (test/e2e/screenshot.ts)
  // and the exact pixel contract in test/screenshot-comparator.ts. The settings below are
  // only a strict tripwire: an accidental toHaveScreenshot must never inherit a permissive
  // Playwright default, and normal runs must never write snapshots.
  expect: { toHaveScreenshot: { threshold: 0, maxDiffPixels: 0 } },
  use: { locale: 'en-US', storageState: { cookies: [], origins: [previewURL, fixtureURL].map(origin => ({ origin, localStorage: [{ name: 'rustx-locale-v1', value: 'en' }] })) }, connectOptions: { wsEndpoint }, baseURL: previewURL, viewport: { width: 1440, height: 1000 }, screenshot: 'only-on-failure', trace: 'retain-on-failure' },
  webServer: [{ command: `pnpm preview --port ${previewPort} --strictPort`, url: previewURL, reuseExistingServer: false }, { command: `pnpm dev --config test/workbench.vite.config.ts --port ${fixturePort} --strictPort`, url: `${fixtureURL}/test/fixtures/foundation.html`, reuseExistingServer: false }],
});
