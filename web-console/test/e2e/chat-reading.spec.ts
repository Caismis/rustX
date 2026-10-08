import { test, expect } from '@playwright/test';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models`;
for (const width of [1440, 390]) test(`small upward gestures own Chat reading at ${width}px`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.goto(fixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.resumeCatalog(); });
  await page.locator('button[data-session-id=A]').click();
  await page.evaluate(() => (window as any).startupFixture.release('session/attach'));
  if (width < 600) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await page.setViewportSize({ width, height: 900 });
  const viewport = page.locator('.conversation-scroll');
  const latest = page.locator('[data-chat-latest]');
  const top = () => viewport.evaluate(el => el.scrollTop);
  const floor = () => viewport.evaluate(el => el.scrollHeight - el.clientHeight);
  const paint = () => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  // Start this gesture scenario at the bottom after the responsive layout settles.
  await paint();
  await viewport.evaluate(el => { el.scrollTop = el.scrollHeight; });
  await expect.poll(async () => (await floor()) - (await top())).toBe(0);
  const box = (await viewport.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  let chunks = 0;
  for (const delta of [1, 6, 12]) {
    const before = await top();
    await page.mouse.wheel(0, -delta);
    await expect(latest).toBeVisible();
    await paint();
    expect(await top()).toBeLessThan(before);
    const reading = await top();
    await page.evaluate(() => (window as any).startupFixture.appendSavedReply());
    await expect(page.getByText(/^Live continuation\./)).toHaveCount(++chunks);
    await paint();
    expect(await top()).toBe(reading);
  }
  await page.screenshot({ path: `/tmp/rustx-chat-reading-${width}.png` });
  await latest.click();
  await expect.poll(async () => (await floor()) - (await top())).toBe(0);
  await expect(latest).toHaveCount(0);
  await page.evaluate(() => (window as any).startupFixture.appendSavedReply());
  await expect(page.getByText(/^Live continuation\./)).toHaveCount(++chunks);
  await expect.poll(async () => (await floor()) - (await top())).toBe(0);
  // Scrolling down to the bottom also restores follow without using the button.
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -6); await expect(latest).toBeVisible();
  await page.mouse.wheel(0, 10000); await expect(latest).toHaveCount(0);
  expect(errors).toEqual([]);
});
