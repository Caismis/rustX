import { defineConfig } from '@playwright/test';
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
  use: { locale: 'en-US', storageState: { cookies: [], origins: ['http://127.0.0.1:5173', 'http://127.0.0.1:5174'].map(origin => ({ origin, localStorage: [{ name: 'rustx-locale-v1', value: 'en' }] })) }, connectOptions: { wsEndpoint }, baseURL: 'http://127.0.0.1:5173', viewport: { width: 1440, height: 1000 }, screenshot: 'only-on-failure', trace: 'retain-on-failure' },
  webServer: [{ command: 'pnpm preview --port 5173 --strictPort', url: 'http://127.0.0.1:5173', reuseExistingServer: false }, { command: 'pnpm dev --port 5174 --strictPort', url: 'http://127.0.0.1:5174/test/fixtures/foundation.html', reuseExistingServer: false }],
});
