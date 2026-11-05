import { defineConfig } from '@playwright/test';

const port = Number(process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
const origin = `http://127.0.0.1:${port}`;
const wsEndpoint = process.env.RUSTX_BROWSER_WS_ENDPOINT;
if (!wsEndpoint && !process.argv.includes('--list')) throw new Error('Set RUSTX_BROWSER_WS_ENDPOINT to the pinned Playwright browser server.');

export default defineConfig({
  testDir: './test/e2e', testMatch: ['geometry-latency.spec.ts', 'geometry-step-latency.spec.ts', 'geometry-performance.spec.ts', 'geometry-progressive.spec.ts', 'geometry-code.spec.ts', 'geometry.spec.ts'],
  workers: 1, timeout: 120_000, repeatEach: 3,
  outputDir: 'test-results/geometry-performance',
  reporter: [['list'], ['json', { outputFile: 'test-results/geometry-performance-report.json' }]],
  use: {
    locale: 'en-US', storageState: { cookies: [], origins: [{ origin, localStorage: [{ name: 'rustx-locale-v1', value: 'en' }] }] },
    connectOptions: { wsEndpoint: wsEndpoint ?? '' }, viewport: { width: 1440, height: 1000 },
    screenshot: 'only-on-failure', trace: 'retain-on-failure',
  },
  webServer: {
    command: `node node_modules/vite/bin/vite.js preview --config test/geometry-performance.vite.config.ts --host 127.0.0.1 --port ${port} --strictPort`,
    url: `${origin}/test/fixtures/geometry.html`, reuseExistingServer: false,
  },
});
