import { test, expect, type Page } from '@playwright/test';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models&subagents`;
async function attach(page: Page, width: number, nested = false) {
  await page.setViewportSize({ width, height: 1000 });
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'en'));
  await page.goto(fixture + (nested ? '&nested-subagents' : ''));
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.allow('session/attach'); f.resumeCatalog(); });
  if (width < 600) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.locator('button[data-session-id=A]').click();
  await expect(page.getByRole('button', { name: 'Subagents', exact: true })).toBeVisible();
  if (width < 600) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
}
for (const theme of ['light', 'dark'] as const) for (const width of [1440, 390]) test(`child conversation layout and resident drafts ${theme} ${width}`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  await attach(page, width);
  const trigger = page.getByRole('button', { name: 'Subagents', exact: true });
  // Hover previews do not steal focus; clicking pins the menu until dismissed.
  await trigger.hover();
  await expect(page.getByRole('menu')).toBeVisible();
  await expect(page.getByRole('menuitem', { name: /Verify findings/ })).toContainText('12K tok');
  await page.screenshot({ path: `/tmp/rustx-subagents-menu-${theme}-${width}.png` });
  await trigger.click();
  await page.mouse.move(10, 500);
  await expect(page.getByRole('menu')).toBeVisible();
  await page.getByRole('menuitem', { name: /Verify findings/ }).click();
  const selected = page.locator('section[data-agent-id="child-1"]');
  const input = page.getByRole('textbox', { name: 'Message Agent Verify findings', exact: true });
  const scroller = page.locator('[data-conversation-scroll]:visible');
  const seat = selected.locator('[data-composer-seat]');
  await expect(selected.locator('[data-composer-dock]')).toContainText('200 tok/s');
  await expect(selected.locator('[data-composer-dock]')).toContainText('10%');
  const atTail = async () => {
    await expect.poll(() => scroller.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThanOrEqual(1);
    await expect.poll(async () => {
      const last = (await selected.locator('[data-chat-anchor-key]').last().boundingBox())!, composer = (await seat.boundingBox())!;
      return last.y + last.height - composer.y;
    }).toBeLessThanOrEqual(1);
  };
  await atTail();
  const small = (await seat.boundingBox())!.height;
  for (let line = 0; line < 5; line++) { await input.pressSequentially('Child guidance'); await input.press('Shift+Enter'); await atTail(); }
  expect((await seat.boundingBox())!.height).toBeGreaterThan(small + 50);
  await page.screenshot({ path: `/tmp/rustx-subagent-input-${theme}-${width}.png` });
  const card = (await selected.locator('[data-composer-card]').boundingBox())!;
  expect(card.x).toBeGreaterThanOrEqual(0); expect(card.x + card.width).toBeLessThanOrEqual(width);
  expect(await input.evaluate(el => getComputedStyle(el).resize)).toBe('none');
  await input.fill('Capped draft\n'.repeat(50)); await atTail();
  expect(await input.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true);
  await input.fill('Keep child draft'); await atTail();
  await scroller.evaluate(el => { el.scrollTop = 300; });
  await expect(scroller).toHaveJSProperty('scrollTop', 300);
  await page.getByRole('button', { name: 'Session A', exact: true }).click();
  await trigger.click(); await page.getByRole('menuitem', { name: /Research sources/ }).click();
  await expect(page.getByRole('button', { name: 'Interrupt', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toHaveCount(0);
  await page.getByRole('textbox', { name: 'Message Agent Research sources', exact: true }).fill('Other child draft');
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Interrupt', exact: true })).toBeEnabled();
  await trigger.click(); await page.getByRole('menuitem', { name: /Verify findings/ }).click();
  await expect(input).toHaveValue('Keep child draft');
  await expect(scroller).toHaveJSProperty('scrollTop', 300);
  // IME confirmation cannot submit; Enter admits through the child's native API.
  await input.dispatchEvent('keydown', { key: 'Enter', isComposing: true });
  expect(await page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'agent/sendMessage').length)).toBe(0);
  await input.press('Enter');
  await expect.poll(() => page.evaluate(() => (window as any).startupFixture.requests().filter((r: any) => r.method === 'agent/sendMessage').map((r: any) => r.params))).toMatchObject([{ agent_id: 'child-1', message: 'Keep child draft' }]);
  await expect(input).toHaveValue('');
  expect(errors).toEqual([]);
});
test('native parent identities drive nested breadcrumbs and sibling menus', async ({ page }) => {
  await attach(page, 1440, true);
  const menus = page.getByRole('button', { name: 'Subagents', exact: true });
  await menus.click();
  expect(await page.getByRole('menuitem').count()).toBe(3);
  await page.getByRole('menuitem', { name: /Verify findings/ }).click();
  await expect(menus).toHaveCount(2);
  await menus.last().click();
  await page.getByRole('menuitem', { name: /Verify original documents/ }).click();
  await expect(page.getByRole('button', { name: 'Verify findings', exact: true })).toBeVisible();
  await expect(page.locator('section[data-agent-id=grandchild]')).toBeVisible();
  await page.getByRole('button', { name: 'Verify findings', exact: true }).click();
  await expect(page.locator('section[data-agent-id=child-1]')).toBeVisible();
});
