import { test, expect } from '@playwright/test';

const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models`;
for (const width of [1440, 390]) test(`typing pushes the conversation above the composer at ${width}px`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.setViewportSize({ width, height: 900 });
  await page.goto(fixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.resumeCatalog(); });
  if (width < 600) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.locator('button[data-session-id=A]').click();
  await page.evaluate(() => (window as any).startupFixture.release('session/attach'));
  await expect(page.getByText('Saved message 19:', { exact: false })).toBeVisible();
  await expect.poll(() => page.locator('[data-conversation-scroll]').evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThanOrEqual(1);
  if (width < 600) {
    await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
    await expect.poll(() => page.locator('[data-conversation-scroll]').evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThanOrEqual(1);
  }
  const scroller = page.locator('[data-conversation-scroll]'), seat = page.locator('[data-composer-seat]');
  const input = page.locator('textarea'), last = page.locator('[data-chat-anchor-key]').last();
  const atTail = async () => {
    await expect.poll(() => scroller.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThanOrEqual(1);
    await expect.poll(async () => {
      const row = (await last.boundingBox())!, composer = (await seat.boundingBox())!;
      return row.y + row.height - composer.y;
    }).toBeLessThanOrEqual(1);
  };
  await atTail();
  const restingHeight = (await seat.boundingBox())!.height;
  await input.click();
  // Real keystrokes, rather than one fill: the former JS measurement collapsed
  // the scroll extent on every edit and detached tail following after line 2.
  for (let line = 0; line < 8; line++) {
    await input.pressSequentially('A growing draft');
    await input.press('Shift+Enter');
    await atTail();
  }
  expect((await seat.boundingBox())!.height).toBeGreaterThan(restingHeight + 100);
  await page.screenshot({ path: `/tmp/rustx-chat-composer-growing-${width}.png` });
  // Pasted text reaches the height cap; further text scrolls inside the draft.
  await input.fill('Long draft\n'.repeat(50));
  await atTail();
  const capped = (await seat.boundingBox())!.height;
  await input.pressSequentially('More content');
  await input.press('Shift+Enter');
  await atTail();
  expect((await seat.boundingBox())!.height).toBe(capped);
  expect(await input.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true);
  await input.fill('Short draft');
  await atTail();
  expect((await seat.boundingBox())!.height).toBe(restingHeight);
  // Rewrapping on viewport changes uses the same natural sizing path.
  await input.fill('A wrapping draft. '.repeat(45));
  await atTail();
  await page.setViewportSize({ width: width === 1440 ? 1100 : 430, height: 900 });
  await atTail();
  // Editing must not bring a reader of older messages back to the tail.
  await scroller.evaluate(el => { el.scrollTop = 600; });
  await expect(scroller).toHaveJSProperty('scrollTop', 600);
  await input.fill('A growing draft\n'.repeat(8));
  await expect(scroller).toHaveJSProperty('scrollTop', 600);
  await input.fill('Short draft');
  await expect(scroller).toHaveJSProperty('scrollTop', 600);
  const body = (await page.locator('#conversation-view').boundingBox())!, track = (await scroller.boundingBox())!;
  expect(track.y + track.height).toBe(body.y + body.height);
  expect(errors).toEqual([]);
});
