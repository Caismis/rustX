import { test, expect, type Page } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
async function open(page: Page, query: string, width: number) {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(`${origin}/test/fixtures/startup.html?existing&${query}`);
  await expect(page).toHaveTitle('rustX');
  await page.evaluate(() => { const fixture = (window as any).startupFixture; fixture.allow('session/summary'); fixture.resumeCatalog(); });
  await page.locator('button[data-session-id=A]').click();
  await page.evaluate(() => (window as any).startupFixture.release('session/attach'));
  if (width < 600) {
    const collapse = page.getByRole('button', { name: 'Collapse Sidebar', exact: true });
    if (await collapse.isVisible()) await collapse.click();
    await page.setViewportSize({ width, height: 844 });
  }
}
for (const width of [1440, 390]) test(`native history completion keeps existing Chat and reading position without a reload prompt (${width})`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.addInitScript(() => localStorage.setItem('rustx-appearance-v1', 'dark'));
  await open(page, 'history-refresh', width);
  const chat = page.getByLabel('Canonical conversation');
  const question = chat.getByText('Run a background task and keep my conversation visible.', { exact: true });
  const call = page.locator('[data-tool-call-id="repair-bash"]');
  await expect(question).toBeAttached(); await expect(call).toBeAttached();
  const reader = page.locator('[data-conversation-scroll]').first();
  await reader.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
  await expect(question).toBeVisible();
  const before = await question.boundingBox();
  await question.evaluate(el => (window as any).savedHistoryQuestion = el);
  await page.evaluate(() => (window as any).startupFixture.advanceHistory());
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.requests().filter((request: any) => request.method === 'session/transcript').length)).toBe(1);
  await expect(chat).not.toContainText('History was refreshed');
  await expect(chat.getByRole('button', { name: /Load earlier|Loading earlier/ })).toHaveCount(0);
  await expect(question).toBeVisible();
  expect(await question.evaluate(el => el === (window as any).savedHistoryQuestion)).toBe(true);
  expect(Math.abs((await question.boundingBox())!.y - before!.y)).toBeLessThanOrEqual(1);
  await page.evaluate(() => (window as any).startupFixture.release('session/transcript'));
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.history().page.entries.find((entry: any) => entry.cursor === '2').tool_calls[0].state.type)).toBe('settled');
  expect(await question.evaluate(el => el === (window as any).savedHistoryQuestion)).toBe(true);
  await expect(chat.getByText('The native task has finished.', { exact: true })).toBeAttached();
  const toggle = call.getByRole('button', { name: /^bash/ });
  if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
  await expect(call.getByText('repaired-result', { exact: true })).toBeVisible();
  await expect(chat.getByRole('button', { name: /Load earlier|Loading earlier/ })).toHaveCount(0);
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Draft is retained during automatic history repair');
  await page.screenshot({ path: `/tmp/rustx-history-repair-${width}.png` });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});
test('Chat retains earlier and latest messages when loading beyond 500 retained records', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await open(page, 'reading-window&long-reading-window', 1440);
  const chat = page.getByLabel('Canonical conversation');
  await expect(chat.getByText('History message 560', { exact: true })).toBeAttached();
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.history()?.initialLoading)).not.toBe(true);
  // The first fetch contains 500 conversational messages plus their Context rows.
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.history()?.page.entries.length)).toBe(1000);
  const reader = page.locator('[data-conversation-scroll]').first();
  await reader.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
  await chat.getByRole('button', { name: 'Load earlier', exact: true }).click();
  await expect(chat.getByText('History message 1', { exact: true })).toBeAttached();
  await expect(chat.getByText('History message 560', { exact: true })).toBeAttached();
  await expect(chat.getByRole('button', { name: 'Load earlier', exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).startupFixture.history().page.entries.length)).toBe(1120);
  await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
  await expect(chat.getByText('History message 560', { exact: true })).toBeVisible();
  await expect(chat.getByText('History message 1', { exact: true })).toBeAttached();
  await expect(chat.getByRole('button', { name: 'Load earlier', exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).startupFixture.history().page.entries.length)).toBe(1120);
  await page.screenshot({ path: '/tmp/rustx-history-long-reading.png' });
  expect(errors).toEqual([]);
});
