import { writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';
test.use({ video: 'on', trace: 'on' });
test('issue420 fixed long response browser recording', async ({ page }, info) => {
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/incremental.html`);
  await page.waitForFunction(() => typeof (window as any).run420 === 'function');
  const metrics = await page.evaluate(() => (window as any).run420());
  writeFileSync(info.outputPath('metrics.json'), JSON.stringify(metrics, null, 2));
  await info.attach('measurements', { body: JSON.stringify(metrics, null, 2), contentType: 'application/json' });
  await page.screenshot({ path: info.outputPath('final.png') });
  expect(metrics.snapshots).toBe(0);
  expect(metrics.historyTop).toBe(210);
  expect(metrics.historyBottomWrites).toBe(0);
  expect(metrics.readingTopAfterGrowth - metrics.readingTopBeforeGrowth).toBe(123);
  expect(metrics.rowReplacements).toBe(0);
});
