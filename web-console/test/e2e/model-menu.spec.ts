import { test, expect } from '@playwright/test';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models`;
for (const width of [1440, 390]) test(`saved conversation model menu keeps composer stable (${width})`, async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ colorScheme: 'dark' });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(fixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.resumeCatalog(); });
  await page.locator('button[data-session-id=A]').click();
  await page.evaluate(() => (window as any).startupFixture.release('session/attach'));
  const trigger = page.getByRole('button', { name: 'Model and profile' });
  await expect(trigger).toBeEnabled();
  if (width < 600) {
    await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(async () => {
      // Resizing can cancel a sidebar transition; either settlement completes layout.
      await Promise.allSettled(document.getAnimations().map(animation => animation.finished));
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    });
  }
  await expect(page.getByText(/Saved message 19:/)).toBeVisible();
  await page.evaluate(() => (window as any).startupFixture.hold('session/models'));
  const geometry = () => page.locator('textarea').evaluate(element => {
    const rect = element.getBoundingClientRect();
    const scroll = document.querySelector('.conversation-scroll')!;
    return { x: rect.x, y: rect.y, height: rect.height, scroll: scroll.scrollTop };
  });
  const before = await geometry();
  for (let opening = 0; opening < 3; opening++) {
    await trigger.click();
    await expect(page.getByText('Reading native models…')).toBeVisible();
    expect(await geometry()).toEqual(before);
    await expect(trigger).toBeEnabled();
    await trigger.click();
    await expect(page.getByRole('menu')).toHaveCount(0);
    await trigger.click();
    await expect(page.getByText('Reading native models…')).toBeVisible();
    await page.evaluate(() => (window as any).startupFixture.release('session/models'));
    await expect(page.getByText('Reading native models…')).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Model', exact: true })).toBeVisible();
    expect(await geometry()).toEqual(before);
    await expect(page.getByRole('menuitem', { name: 'Model', exact: true })).toBeFocused();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('menu')).toHaveCount(2);
    await expect(page.getByRole('menu').last()).toBeVisible();
    await page.screenshot({ path: `test-results/model-menu-${width}.png` });
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(await geometry()).toEqual(before);
  }
  const reads = await page.evaluate(() => (window as any).startupFixture.requests().filter((request: any) => request.method === 'session/models').length);
  expect(reads).toBe(3);
  expect(errors).toEqual([]);
});
