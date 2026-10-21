import { test, expect } from '@playwright/test';

const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const width of [1440, 390]) test(`deferred reply survives settlement and checkpoint refresh at ${width}px`, async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${fixtureOrigin}/test/fixtures/agent.html?mode=deferred-reply`);
  await expect(page).toHaveTitle('rustX Agent reference');
  await expect(page).toHaveURL(/agent\.html\?mode=deferred-reply$/);
  const answer = page.getByRole('heading', { name: 'Deferred answers remain visible' });
  await expect(answer).toBeVisible();
  await page.evaluate(() => window.deferredReplyFixture.settle());
  const worked = page.getByRole('button', { name: 'Worked', exact: true });
  await expect(worked).toBeVisible();
  await expect(answer).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.deferredReplyFixture.reads())).toBeGreaterThanOrEqual(1);
  await page.evaluate(() => window.deferredReplyFixture.refresh());
  await expect(answer).toBeVisible();
  await expect(page.getByText('The complete answer survives native settlement and a fresh checkpoint read.')).toBeVisible();
  await worked.click();
  await page.getByRole('button', { name: 'Analysis completed', exact: true }).click();
  await page.getByRole('button', { name: /^Reasoning/ }).click();
  await expect(page.getByText('The full reasoning is available from the canonical content reference.')).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.deferredReplyFixture.reads())).toBeGreaterThanOrEqual(2);
  await worked.click();
  await answer.scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
  const screenshot = testInfo.outputPath(`deferred-reply-${width}.png`);
  await page.screenshot({ path: screenshot });
  await testInfo.attach('settled reply', { path: screenshot, contentType: 'image/png' });
});
