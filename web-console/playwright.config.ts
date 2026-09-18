import { defineConfig } from '@playwright/test';
const wsEndpoint = process.env.RUSTX_BROWSER_WS_ENDPOINT;
if (!wsEndpoint) throw new Error('Use pnpm test:e2e or pnpm test:e2e:update: references require the pinned browser container.');
const references = /(?:agent|foundation|shell|settings-presentation)\.spec\.ts$/;
export default defineConfig({
  testDir: './test/e2e', testMatch: '*.spec.ts', workers: 1, timeout: 120_000,
  // Keep reference rasterization independent of native tests' browser history.
  // Projects get separate workers/browsers in both update and normal runs.
  // The unnamed reference project retains the existing snapshot paths.
  projects: [{ testMatch: references }, { name: 'native', testIgnore: references }],
  updateSnapshots: 'none',
  expect: { toHaveScreenshot: { threshold: 0, maxDiffPixels: 0 } },
  use: { connectOptions: { wsEndpoint }, baseURL: 'http://127.0.0.1:5173', viewport: { width: 1440, height: 1000 }, screenshot: 'only-on-failure', trace: 'retain-on-failure' },
  webServer: [{ command: 'pnpm preview --port 5173 --strictPort', url: 'http://127.0.0.1:5173', reuseExistingServer: false }, { command: 'pnpm dev --port 5174 --strictPort', url: 'http://127.0.0.1:5174/test/fixtures/foundation.html', reuseExistingServer: false }],
});
