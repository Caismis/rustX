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
  await expect(page.getByRole('tree', { name: 'Subagents', exact: true })).toBeVisible();
  await expect(page.getByRole('treeitem', { name: /Verify findings/ })).toContainText('12K tok');
  await page.screenshot({ path: `/tmp/rustx-subagents-menu-${theme}-${width}.png` });
  await trigger.click();
  await page.mouse.move(10, 500);
  await expect(page.getByRole('tree', { name: 'Subagents', exact: true })).toBeVisible();
  await page.getByRole('treeitem', { name: /Verify findings/ }).click();
  const selected = page.locator('section[data-agent-id="child-1"]');
  const input = page.getByRole('textbox', { name: 'Message Agent Verify findings', exact: true });
  const scroller = page.locator('[data-conversation-scroll]:visible');
  const seat = selected.locator('[data-composer-seat]');
  await expect(selected.locator('[data-composer-dock]')).toContainText('200 tok/s');
  await expect(selected.locator('[data-composer-dock]')).toContainText('10%');
  const singleStatsRow = async () => {
    const dock = selected.locator('[data-composer-dock]');
    const bounds = (await dock.boundingBox())!;
    const buttons = dock.getByRole('button');
    await expect(buttons).toHaveCount(3);
    const rects = await buttons.evaluateAll(elements => elements.map(el => {
      const { x, y, width, height } = el.getBoundingClientRect();
      return { x, y, width, height };
    }));
    for (const rect of rects) {
      expect(Math.abs(rect.y + rect.height / 2 - (rects[2].y + rects[2].height / 2))).toBeLessThanOrEqual(1);
      expect(rect.x).toBeGreaterThanOrEqual(bounds.x);
      expect(rect.x + rect.width).toBeLessThanOrEqual(bounds.x + bounds.width + 1);
    }
    await expect(buttons.last()).toHaveText('10%');
  };
  await singleStatsRow();
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
  await singleStatsRow();
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
  await trigger.click(); await page.getByRole('treeitem', { name: /Research sources/ }).click();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: /^(Send|Steer)$/, exact: true })).toHaveCount(0);
  await page.getByRole('textbox', { name: 'Message Agent Research sources', exact: true }).fill('Other child draft');
  await expect(page.getByRole('button', { name: /^(Send|Steer)$/, exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0);
  await trigger.click(); await page.getByRole('treeitem', { name: /Verify findings/ }).click();
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
  expect(await page.getByRole('tree', { name: 'Subagents', exact: true }).getByRole('treeitem').count()).toBe(3);
  await page.getByRole('treeitem', { name: /Verify findings/ }).click();
  await expect(menus).toHaveCount(2);
  await menus.last().click();
  await page.getByRole('treeitem', { name: /Verify original documents/ }).click();
  await expect(page.getByRole('button', { name: 'Verify findings', exact: true })).toBeVisible();
  await expect(page.locator('section[data-agent-id=grandchild]')).toBeVisible();
  await page.getByRole('button', { name: 'Verify findings', exact: true }).click();
  await expect(page.locator('section[data-agent-id=child-1]')).toBeVisible();
});

for (const width of [1440, 390]) test(`tree expansion, keyboard navigation and sidebar conversations retain native identity and drafts at ${width}`,  async ({ page }) => {
  page.setDefaultTimeout(10_000);
  await attach(page, width, true);
  const trigger = page.getByRole('button', { name: 'Subagents', exact: true });
  await trigger.focus(); await trigger.press('ArrowDown');
  const first = page.getByRole('treeitem', { name: /^Research sources / });
  await expect(first).toBeFocused(); await first.press('ArrowDown');
  const branch = page.getByRole('treeitem', { name: /^Verify findings / });
  await expect(branch).toBeFocused(); await branch.press('ArrowRight');
  await expect(branch).toHaveAttribute('aria-expanded', 'true');
  await branch.press('ArrowDown');
  const leaf = page.getByRole('treeitem', { name: /^Verify original documents / });
  await expect(leaf).toBeFocused(); await leaf.press('Escape');
  await expect(trigger).toBeFocused(); await expect(page.getByRole('tree', { name: 'Subagents', exact: true })).toHaveCount(0);
  await trigger.click();
  await page.getByRole('button', { name: 'Open Verify findings in sidebar', exact: true }).click();
  const sidebar = page.locator('[data-workbench]');
  const child = sidebar.locator('section[data-agent-id="child-1"]');
  await expect(child).toBeVisible();
  await expect(sidebar).not.toContainText('Select a Session connected to a local Product Host');
  expect(await sidebar.locator('[data-dockkit-strip]').evaluate(el => el.getBoundingClientRect().top)).toBe(0);
  await expect(page.locator('#conversation-view section[data-agent-id]:visible')).toHaveCount(0);
  const input = child.getByRole('textbox', { name: 'Message Agent Verify findings' });
  await input.fill('Sidebar draft');
  if (width < 600) await sidebar.getByRole('button', { name: 'Toggle workspace panel', exact: true }).click();
  await trigger.click();
  await page.getByRole('button', { name: 'Open Research sources in sidebar', exact: true }).click();
  await expect(sidebar.locator('section[data-agent-id="child-0"]')).toBeVisible();
  await sidebar.getByRole('tab', { name: 'Verify findings', exact: false }).click();
  await expect(input).toHaveValue('Sidebar draft');
  await expect(child).toContainText('10%');
  await page.screenshot({ path: `/tmp/rustx-subagent-aside-audit-${width}.png` });
});
