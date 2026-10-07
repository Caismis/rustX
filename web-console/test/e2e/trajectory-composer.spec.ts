import { test, expect } from '@playwright/test';
import { PNG } from 'pngjs';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&trajectory`;
for (const theme of ['light', 'dark'] as const) for (const width of [1440, 390]) test(`trajectory composer fade ${theme} ${width}`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  await page.goto(fixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.resumeCatalog(); });
  await page.locator('button[data-session-id=A]').click();
  await page.evaluate(() => (window as any).startupFixture.release('session/attach'));
  if (width < 600) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await page.setViewportSize({ width, height: 900 });
  await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
  const ledger = page.locator('[data-trajectory-scroll]'), seat = page.locator('[data-composer-seat]');
  await expect(ledger).toBeVisible();
  for (const draft of ['', 'A growing draft\n'.repeat(8)]) {
    await page.locator('textarea').fill(draft);
    await expect.poll(async () => ledger.evaluate(el => parseFloat(getComputedStyle(el).paddingBottom))).toBe((await seat.boundingBox())!.height + 16);
    const last = ledger.getByText('Saved trace 159', { exact: true });
    await expect(last).toBeVisible();
    const seatBox = (await seat.boundingBox())!, body = (await page.locator('#conversation-view').boundingBox())!;
    expect((await ledger.boundingBox())!.y + (await ledger.boundingBox())!.height).toBe(body.y + body.height);
    expect((await last.boundingBox())!.y + (await last.boundingBox())!.height).toBeLessThanOrEqual(seatBox.y - 16);
    const png = PNG.sync.read(await page.screenshot({ ...(draft === '' ? { path: `/tmp/rustx-trajectory-resting-${theme}-${width}.png` } : {}) }));
    const sample = (offset: number) => png.data[(Math.floor(seatBox.y + offset) * png.width + Math.floor(body.x + 4)) * 4]!;
    const top = sample(0), middle = sample(18), solid = sample(40);
    expect(await seat.evaluate(el => getComputedStyle(el).backgroundImage)).toContain('36px');
    if (theme === 'dark') {
      expect(middle).toBeGreaterThan(Math.min(top, solid));
      expect(middle).toBeLessThan(Math.max(top, solid));
      expect(sample(36)).toBe(solid);
    } else {
      // Harness light tokens use white for both bg-base and bg-layer-1.
      expect(middle).toBe(255);
    }
  }
  await page.screenshot({ path: `/tmp/rustx-trajectory-fade-${theme}-${width}.png` });
  await ledger.evaluate(el => { el.scrollTop = 0; });
  await expect(ledger.getByText('Saved trace 0', { exact: true })).toBeVisible();
  await page.locator('textarea').fill('Short draft');
  await expect.poll(async () => ledger.evaluate(el => parseFloat(getComputedStyle(el).paddingBottom))).toBe((await seat.boundingBox())!.height + 16);
  expect(await ledger.evaluate(el => el.scrollTop)).toBe(0);
  await page.getByRole('tab', { name: 'Chat', exact: true }).click();
  await expect(page.locator('[data-conversation-composer-overlay]')).toHaveCount(0);
  expect(await seat.evaluate(el => getComputedStyle(el).position)).toBe('sticky');
  expect(errors).toEqual([]);
});
