import { test, expect } from '@playwright/test';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models`;
for (const width of [1440, 390]) test(`switching Chat after frame callbacks positions the first paint at ${width}px`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(fixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.allow('thread/resume'); f.resumeCatalog(); });
  await page.locator('button[data-session-id=A]').click();
  await expect(page.getByText(/^Saved message 19:/)).toBeAttached();
  if (width < 600) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await page.setViewportSize({ width, height: 900 });
  for (const id of ['B', 'A', 'B', 'A', 'B', 'A']) {
    const expand = page.getByRole('button', { name: 'Home', exact: true });
    if (await expand.isVisible()) await expand.click();
    await expect(page.locator(`button[data-session-id="${id}"]`)).toBeAttached();
    // A navigation can commit after this frame's animation callbacks. Inspect
    // the first paint, before a newly requested animation frame can hide a jump.
    const firstPaint = await page.evaluate(id => new Promise<{ title: string; remaining: number }>(resolve => {
      requestAnimationFrame(() => {
        document.querySelector<HTMLButtonElement>(`button[data-session-id="${id}"]`)!.click();
        setTimeout(() => {
          const port = document.querySelector<HTMLElement>('[data-conversation-scroll]')!;
          resolve({ title: document.title, remaining: port.scrollHeight - port.clientHeight - port.scrollTop });
        }, 0);
      });
    }), id);
    expect(firstPaint.title).toBe(`Session ${id} — rustX`);
    expect(firstPaint.remaining).toBe(0);
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
  }
  await page.screenshot({ animations: 'disabled', path: `/tmp/rustx-chat-switch-${width}.png` });
  expect(errors).toEqual([]);
});
for (const width of [1440, 390]) test(`opening Chat preserves history through its bounded checkpoint at ${width}px`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${fixture}&opening-history`);
  await page.evaluate(() => {
    const f = (window as any).startupFixture;
    f.allow('session/summary'); f.resumeCatalog(); f.hold('session/checkpoint'); f.hold('session/transcript');
  });
  await page.locator('button[data-session-id=A]').click();
  const first = page.getByText(/^Saved message 0:/);
  await expect(first).toBeAttached();
  if (width < 600) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await page.setViewportSize({ width, height: 900 });
  const port = page.locator('[data-conversation-scroll]').first();
  await first.evaluate(element => {
    const port = element.closest('[data-conversation-scroll]')!;
    const observer = new MutationObserver(() => { if (!element.isConnected) port.setAttribute('data-history-removed', 'true'); });
    observer.observe(port, { childList: true, subtree: true });
    (window as any).stopOpeningObserver = () => observer.disconnect();
  });
  await page.evaluate(() => (window as any).startupFixture.release('thread/resume'));
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.observation('A').state)).toBe('checkpointLoading');
  await expect(first).toBeAttached();
  await page.evaluate(() => (window as any).startupFixture.release('session/checkpoint'));
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.observation('A').state)).toBe('following');
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(port.locator('[data-chat-anchor-key]')).toHaveCount(20);
  await expect(port).not.toHaveAttribute('data-history-removed', 'true');
  expect(await page.evaluate(() => (window as any).startupFixture.requests().filter((request: any) => request.method === 'session/transcript'))).toEqual([]);
  expect(await port.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBe(0);
  await expect(page.locator('[data-chat-latest]')).toHaveCount(0);
  await page.screenshot({ path: `/tmp/rustx-chat-opening-${width}.png` });
  await page.evaluate(() => (window as any).stopOpeningObserver());
  expect(errors).toEqual([]);
});
for (const width of [1440, 390]) test(`Harness follow threshold and detached Chat reading at ${width}px`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.goto(fixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.resumeCatalog(); });
  await page.locator('button[data-session-id=A]').click();
  await page.evaluate(() => (window as any).startupFixture.release('thread/resume'));
  if (width < 600) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await page.setViewportSize({ width, height: 900 });
  const viewport = page.locator('[data-conversation-scroll]').first();
  const latest = page.locator('[data-chat-latest]');
  const top = () => viewport.evaluate(el => el.scrollTop);
  const floor = () => viewport.evaluate(el => el.scrollHeight - el.clientHeight);
  const paint = () => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  const scrollUp = async (delta: number) => {
    await viewport.evaluate(el => {
      el.removeAttribute('data-test-scroll-settled');
      el.addEventListener('scrollend', () => el.setAttribute('data-test-scroll-settled', 'true'), { once: true });
    });
    await page.mouse.wheel(0, -delta);
    await expect(viewport).toHaveAttribute('data-test-scroll-settled', 'true');
  };
  // Start this gesture scenario at the bottom after the responsive layout settles.
  await paint();
  await viewport.evaluate(el => { el.scrollTop = el.scrollHeight; });
  await expect.poll(async () => (await floor()) - (await top())).toBe(0);
  const box = (await viewport.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  // Harness samples at scrollend and keeps following within 25px of the tail.
  for (const delta of [1, 6, 12]) {
    const before = await top();
    await scrollUp(delta);
    expect(await top()).toBeLessThan(before);
    await expect(latest).toHaveCount(0);
  }
  expect((await floor()) - (await top())).toBe(19);
  let chunks = 0;
  for (const delta of [32, 6, 12]) {
    await scrollUp(delta);
    await expect(latest).toBeVisible();
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
  await scrollUp(40); await expect(latest).toBeVisible();
  await page.mouse.wheel(0, 10000); await expect(latest).toHaveCount(0);
  expect(errors).toEqual([]);
});
