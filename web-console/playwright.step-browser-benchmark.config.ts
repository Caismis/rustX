import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './test/e2e',
  testMatch: 'step-browser.bench.ts',
  workers: 1,
  timeout: 0,
  reporter: 'list',
});
