import { test, expect, type Page } from '@playwright/test';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models&cold-model`;

const geometry = (page: Page) => page.locator('textarea').evaluate(element => {
 const rect = element.getBoundingClientRect(), scroll = document.querySelector('[data-conversation-scroll]')!;
 return { x: rect.x, y: rect.y, height: rect.height, scroll: scroll.scrollTop };
});
/** Observe painted frames while a real client write/read is held by the fixture. */
async function stablePendingFrames(page: Page) {
 return page.evaluate(async () => {
  const original = document.querySelector('[data-model-menu]')!, rows = [...original.querySelectorAll('[role="menuitemradio"]')];
  const rect = original.getBoundingClientRect();
  const samples = [];
  for (let i = 0; i < 12; i++) {
   await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
   const card = document.querySelector('[data-model-menu]'), box = card?.getBoundingClientRect();
   samples.push({ same: card === original && rows.every(row => row.isConnected), stable: box?.x === rect.x && box.y === rect.y && box.width === rect.width && box.height === rect.height });
  }
  return samples;
 });
}

for (const width of [1440, 390]) test(`DSH model and effort panes retain layout through native confirmation (${width})`, async ({ page }) => {
 page.setDefaultTimeout(15_000);
 const zh = width === 390, copy = (en: string, cn: string) => zh ? cn : en;
 await page.addInitScript(zh => { localStorage.setItem('rustx-locale-v1', zh ? 'zh' : 'en'); localStorage.setItem('rustx-appearance-v1', zh ? 'dark' : 'light'); }, zh);
 await page.setViewportSize({ width: 1440, height: 900 });
 const errors: string[] = [];
 page.on('pageerror', error => errors.push(error.message));
 page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
 await page.goto(fixture);
 await expect(page).toHaveTitle('rustX startup ownership fixture');
 await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.resumeCatalog(); });
 await page.locator('button[data-session-id=A]').click();
 await page.evaluate(() => (window as any).startupFixture.release('session/attach'));
 const trigger = page.locator('[data-model-select]');
 await expect(trigger).toBeEnabled();
 if (width < 600) {
  await page.getByRole('button', { name: '收起侧边栏', exact: true }).click();
  await page.setViewportSize({ width, height: 900 });
  await page.evaluate(async () => { await Promise.allSettled(document.getAnimations().map(animation => animation.finished)); });
 }
 await expect(page.getByText(/Saved message 19:/)).toBeVisible();
 // The saved fixture's old model is absent from the new catalog: choose a
 // published model first, using the same native client path as every switch.
 await trigger.click();
 if (await page.getByRole('menuitem', { name: copy('Model', '模型'), exact: true }).isVisible()) await page.getByRole('menuitem', { name: copy('Model', '模型'), exact: true }).click();
 await page.getByRole('menuitemradio', { name: 'fixture/native', exact: true }).click();
 await expect(trigger).toHaveAttribute('aria-expanded', 'false');
 await expect(trigger).toContainText('high');
 const before = await geometry(page);
 const reads = await page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'session/models').length);
 for (let opening = 0; opening < 2; opening++) {
  await trigger.click(); const card = page.locator('[data-model-menu]');
  await expect(card).toHaveAttribute('data-pane', 'root');
  await expect(card.getByRole('menuitem')).toHaveCount(2);
  await expect(page.getByText(copy('Reading native models…', '正在读取原生模型…'))).toHaveCount(0);
  expect(await geometry(page)).toEqual(before);
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: copy('Model', '模型'), exact: true })).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(card).toHaveAttribute('data-pane', 'model'); await expect(page.getByRole('menu')).toHaveCount(1);
  await expect(page.getByRole('menuitemradio', { name: 'fixture/native', exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(card).toHaveAttribute('data-pane', 'root');
  await page.keyboard.press('Escape'); await expect(trigger).toBeFocused();
  expect(await geometry(page)).toEqual(before);
 }
 expect(await page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'session/models').length)).toBe(reads);
 const switchHeld = async (pane: 'model' | 'effort', value: string, confirmed: string) => {
  await trigger.click();
  await page.getByRole('menuitem', { name: pane === 'model' ? copy('Model', '模型') : copy('Thinking effort', '思考强度'), exact: true }).click();
  const card = page.locator('[data-model-menu]');
  if (pane === 'effort') {
   await expect(card.getByRole('menuitemradio')).toHaveCount(2);
   await expect(card).not.toContainText(/模型.*预设|Model default profile/);
  }
  await page.evaluate(() => { (window as any).startupFixture.hold('session/setModel'); (window as any).startupFixture.hold('session/checkpoint'); });
  const writes = await page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'session/setModel').length);
  await page.getByRole('menuitemradio', { name: value, exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'session/setModel').length)).toBe(writes + 1);
  await expect(trigger).toHaveAttribute('aria-busy', 'true'); await expect(trigger).toContainText(confirmed);
  await expect(trigger).toHaveCSS('outline-style', 'none');
  expect((await stablePendingFrames(page)).every(sample => sample.same && sample.stable)).toBe(true);
  expect(await geometry(page)).toEqual(before);
  await page.screenshot({ path: `/tmp/rustx-model-${pane}-${width}.png` });
  await page.evaluate(() => (window as any).startupFixture.release('session/setModel'));
  await expect(trigger).toContainText(confirmed); await expect(card).toBeVisible();
  expect((await stablePendingFrames(page)).every(sample => sample.same && sample.stable)).toBe(true);
  await page.evaluate(() => (window as any).startupFixture.release('session/checkpoint'));
  await expect(card).toHaveCount(0); await expect(trigger).toHaveAttribute('aria-busy', 'false'); await expect(trigger).toBeFocused();
  await page.evaluate(() => { (window as any).startupFixture.allow('session/setModel'); (window as any).startupFixture.allow('session/checkpoint'); });
  expect(await geometry(page)).toEqual(before);
 };
 await switchHeld('effort', 'low', 'high'); await expect(trigger).toContainText('low');
 await switchHeld('model', 'fixture/second', 'fixture/native'); await expect(trigger).toContainText('fixture/second');
 expect(await page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'turn/start').length)).toBe(0);
 expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
 await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});
