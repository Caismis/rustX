import { test, expect } from '@playwright/test';

const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const deferred of [false, true]) for (const width of [1440, 390]) test(`Turn delivery placement, summary and reload ${width}px deferred=${deferred}`, async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript(() => localStorage.setItem('rustx-appearance-v1', 'dark'));
  await page.goto(`${fixtureOrigin}/test/fixtures/agent.html?mode=turn-deliveries${deferred ? '&deferred=1' : ''}`);
  await expect(page).toHaveTitle('Session A — rustX');
  await expect(page).toHaveURL(/agent\.html\?mode=turn-deliveries/);
  await expect(page.getByText('Finalizing the report and instructions.')).toBeVisible();
  await expect(page.locator('[data-delivery-card]')).toHaveCount(0);
  await page.evaluate(() => window.turnDeliveriesFixture.settle());
  const cards = page.locator('[data-turn-deliveries]');
  await expect(cards).toHaveCount(1);
  await expect(cards.locator('[data-delivery-card]')).toHaveCount(4);
  const ordered = async () => page.evaluate(() => {
    const prose = [...document.querySelectorAll('p')].find(node => node.textContent === 'All checks passed.')!;
    const cards = document.querySelector('[data-turn-deliveries]')!, actions = document.querySelector('[data-turn-tail]')!;
    return !!(prose.compareDocumentPosition(cards) & Node.DOCUMENT_POSITION_FOLLOWING)
      && !!(cards.compareDocumentPosition(actions) & Node.DOCUMENT_POSITION_FOLLOWING);
  });
  expect(await ordered()).toBe(true);
  await cards.getByRole('button', { name: 'Show all 7 delivered files' }).click();
  await expect(cards.locator('[data-presented-name]')).toHaveText(['report.md', 'b.txt', 'c.rs', 'd.png', 'e.bin', 'f.txt', 'README.md']);
  await expect(cards.locator('[data-presented-description]').first()).toContainText('Latest report');
  await cards.getByRole('button', { name: 'Collapse delivered files' }).click();
  await page.evaluate(() => window.turnDeliveriesFixture.refresh());
  await expect(cards).toBeVisible(); expect(await ordered()).toBe(true);
  await page.evaluate(() => window.turnDeliveriesFixture.nextQuestion());
  await expect(page.getByText('Continue with the next task.')).toBeVisible();
  expect(await page.evaluate(() => !!(document.querySelector('[data-turn-tail]')!.compareDocumentPosition(
    [...document.querySelectorAll('[data-message-body]')].find(node => node.textContent === 'Continue with the next task.')!) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
  if (!deferred) {
    const worked = page.getByRole('button', { name: 'Worked', exact: true });
    await worked.click(); await expect(cards).toBeVisible(); await worked.click(); await expect(cards).toBeVisible();
  }
  await page.getByRole('heading', { name: 'Delivery complete' }).scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  const screenshot = testInfo.outputPath(`turn-deliveries-${width}-${deferred ? 'head' : 'inline'}.png`);
  await page.screenshot({ path: screenshot }); await testInfo.attach('Turn delivery placement', { path: screenshot, contentType: 'image/png' });
});
