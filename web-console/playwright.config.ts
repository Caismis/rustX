import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './test/e2e', testMatch: '*.spec.ts', workers: 1, timeout: 120_000,
  use: { baseURL: 'http://127.0.0.1:5173', viewport: { width: 1440, height: 1000 }, screenshot: 'only-on-failure', trace: 'retain-on-failure' },
  webServer: [{ command: 'pnpm preview --port 5173 --strictPort', url: 'http://127.0.0.1:5173', reuseExistingServer: false }, { command: 'pnpm dev --port 5174 --strictPort', url: 'http://127.0.0.1:5174/test/fixtures/foundation.html', reuseExistingServer: false }],
});
