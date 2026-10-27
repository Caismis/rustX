import { expect, test } from '@playwright/test';
declare global { interface Window { continuity: { publish(): void; changeGoal(): void; reads(): number; advancePrefix(): void; finishPrefix(): void; setRows(count: number): void } } }
const origin = 'http://127.0.0.1:' + (process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
test('a deferred Goal update keeps expanded hydrated Todo rows in place', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  await page.getByRole('button', { name: /To-dos/ }).click();
  const rows = page.locator('[data-task-id]'); await expect(rows).toHaveCount(3);
  await rows.evaluateAll(elements => elements.forEach(element => element.setAttribute('data-retained-row', 'true')));
  const positions = await rows.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().top));
  await page.evaluate(() => window.continuity.changeGoal());
  await expect(page.locator('[data-retained-row]')).toHaveCount(3);
  expect(await rows.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().top))).toEqual(positions);
  expect(await page.evaluate(() => window.continuity.reads())).toBe(1);
  await expect(page.getByRole('button', { name: /To-dos/ })).toHaveAttribute('aria-expanded', 'true');
});
for (const mode of ['standard', 'compact']) test(`published reasoning keeps its expanded DOM and reading scroll in ${mode}`, async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?mode=' + mode);
  await page.locator('[data-step-process] > button').click();
  await page.locator('[data-variant="think"] [data-disclosure-row]').click();
  const body = page.locator('[data-step-process-body]');
  await body.evaluate(element => { element.setAttribute('data-retained-body', 'true'); element.scrollTop = 96; });
  await expect.poll(() => body.evaluate(element => element.scrollTop)).toBe(96);
  const before = await body.evaluate(element => element.querySelector('[data-variant="think"]')!.getBoundingClientRect().top - element.getBoundingClientRect().top);
  await page.evaluate(() => window.continuity.publish());
  await expect(page.locator('[data-step-process] > button')).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('[data-retained-body]')).toHaveCount(1);
  await expect(page.locator('[data-variant="think"]')).toHaveAttribute('data-expanded', 'true');
  expect(await body.evaluate(element => element.scrollTop)).toBe(96);
  // Publication can add the native Attempt header; the inner reading offset stays fixed.
  const offset = await body.evaluate(element => element.querySelector('[data-variant="think"]')!.getBoundingClientRect().top - element.getBoundingClientRect().top);
  expect(offset).toBe(before);
});
test('an advancing exact live reference preserves released text and its reading node during the read', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  const body = page.locator('[data-live-prefix]');
  await expect.poll(() => body.textContent().then(text => text?.length)).toBe(30000);
  await body.evaluate(element => { element.setAttribute('data-retained-prefix', 'true'); element.scrollTop = 96; });
  await page.evaluate(() => window.continuity.advancePrefix());
  await expect(page.locator('[data-retained-prefix]')).toHaveCount(1);
  expect((await body.textContent())?.length).toBe(30000); expect(await body.evaluate(element => element.scrollTop)).toBe(96);
  await page.evaluate(() => window.continuity.finishPrefix());
  await expect.poll(() => body.textContent().then(text => text?.length)).toBe(30020);
  await expect(page.locator('[data-retained-prefix]')).toHaveCount(1);
  expect(await body.evaluate(element => element.scrollTop)).toBe(96);
});
test('off-tail Trace reading survives append, fold and filter transitions across the virtualization threshold', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(element => { element.scrollTop = 900; element.dispatchEvent(new Event('scroll')); });
  for (const count of [101, 90, 101]) {
    await page.evaluate(count => window.continuity.setRows(count), count);
    await expect(ledger).toHaveAttribute('aria-rowcount', String(count));
    expect(await ledger.evaluate(element => element.scrollTop)).toBe(900);
  }
});
test('Trace still follows appends at the tail when virtualization is enabled', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(element => { element.scrollTop = element.scrollHeight; element.dispatchEvent(new Event('scroll')); });
  await page.evaluate(() => window.continuity.setRows(101));
  await expect(ledger).toHaveAttribute('aria-rowcount', '101');
  await expect.poll(() => ledger.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(2);
});
