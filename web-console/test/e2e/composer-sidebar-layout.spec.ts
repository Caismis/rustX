import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const locale of ['en', 'zh']) test(`composer follows its pane width with both sidebars (${locale})`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', event => { if (event.type() === 'error') errors.push(event.text()); });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ colorScheme: locale === 'zh' ? 'light' : 'dark', reducedMotion: 'reduce' });
  await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
  await page.goto(`${origin}/test/fixtures/workbench.html?composer-layout`);
  await expect(page).toHaveTitle('Workspace panel');
  const card = page.locator('[data-composer-card]:visible'), model = card.locator('[data-model-select]');
  const input = card.locator('textarea'), send = card.locator('[data-composer-primary]');
  const add = card.locator('button[aria-haspopup="listbox"]');
  const oneRow = async () => {
    await expect.poll(async () => Math.abs((await add.boundingBox())!.y + 14 - ((await send.boundingBox())!.y + 19))).toBeLessThanOrEqual(1);
    await expect(page.locator('[data-composer-dock]:visible')).toContainText('10%');
    const box = (await card.boundingBox())!, dock = (await page.locator('[data-composer-dock]:visible').boundingBox())!;
    expect(dock.y).toBeGreaterThanOrEqual(box.y + box.height);
    expect(dock.y + dock.height).toBeLessThanOrEqual(page.viewportSize()!.height);
    expect(dock.width).toBeLessThanOrEqual((await page.locator('.conversation-panel:visible').boundingBox())!.width);
    for (const control of [add, card.locator('button[aria-haspopup="menu"]').first(), model, send]) {
      const rect = (await control.boundingBox())!;
      expect(rect.x).toBeGreaterThanOrEqual(box.x);
      expect(rect.x + rect.width).toBeLessThanOrEqual(box.x + box.width);
    }
  };
  await expect(model).toContainText('DeepSeek/deepseek-flash');
  await oneRow();
  const height = (await card.boundingBox())!.height;
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await expect(panel).toBeVisible();
  await oneRow();
  expect((await card.boundingBox())!.height).toBe(height);
  await panel.getByRole('button', { name: locale === 'zh' ? /工作区文件/ : /Workspace files/ }).click();
  await panel.getByRole('button', { name: 'docs', exact: true }).click();
  await panel.getByRole('button', { name: 'guide.md', exact: true }).click();
  await expect(panel.getByRole('heading', { name: 'Rendered document' })).toBeVisible();
  // Resize the actual pane, without changing the desktop viewport.
  const handle = page.locator('[data-side="rightbar"]');
  const resize = async (x: number) => {
    const rect = (await handle.boundingBox())!;
    await page.mouse.move(rect.x + rect.width / 2, 500);
    await page.mouse.down(); await page.mouse.move(x, 500, { steps: 12 }); await page.mouse.up();
    await oneRow();
    expect((await card.boundingBox())!.height).toBe(height);
  };
  await resize(680); // 400px main pane: permissions lose their label, not their control.
  await expect.poll(() => card.locator('button[aria-haspopup="menu"]').first().evaluate(el => getComputedStyle(el.querySelectorAll('span')[1]!).display)).toBe('none');
  expect((await model.boundingBox())!.width).toBeLessThanOrEqual((await card.locator('[data-composer-controls]').boundingBox())!.width * .45);
  await card.locator('button[aria-haspopup="menu"]').first().click();
  await expect(page.getByRole('menu').first()).toBeVisible();
  await page.keyboard.press('Escape');
  await input.fill('A retained draft while panels resize.');
  await model.click();
  await page.getByRole('menuitem', { name: locale === 'zh' ? '模型' : 'Model', exact: true }).hover();
  await page.getByRole('menuitem', { name: 'Short', exact: true }).click();
  await expect(model).toContainText('Short');
  await oneRow();
  await resize(980);
  await expect.poll(() => model.evaluate(el => getComputedStyle(el.querySelector('span')!).display)).not.toBe('none');
  await expect(input).toHaveValue('A retained draft while panels resize.');
  await page.locator('[data-preview-toggle]').click(); await oneRow();
  await page.locator('[data-preview-toggle]').click(); await oneRow();
  await resize(680);
  await model.click();
  await page.getByRole('menuitem', { name: locale === 'zh' ? '模型' : 'Model', exact: true }).hover();
  await page.getByRole('menuitem', { name: 'DeepSeek/deepseek-flash', exact: true }).click();
  await expect(model).toContainText('DeepSeek/deepseek-flash');
  await oneRow();
  await page.screenshot({ path: `/tmp/rustx-composer-sidebar-${locale}.png` });
  await page.getByRole('button', { name: locale === 'zh' ? '收起侧边栏' : 'Collapse Sidebar', exact: true }).click();
  await oneRow();
  await page.getByRole('button', { name: locale === 'zh' ? '打开侧边栏' : 'Expand Sidebar', exact: true }).click();
  await oneRow();
  // A multiline draft reserves natural space above the sticky seat.
  await input.fill('A growing draft\n'.repeat(8));
  const last = page.locator('[data-chat-anchor-key]').last(), seat = page.locator('[data-composer-seat]:visible');
  await expect.poll(async () => { const row = (await last.boundingBox())!, box = (await seat.boundingBox())!; return row.y + row.height - box.y; }).toBeLessThanOrEqual(1);
  await input.fill('');
  await page.locator('[data-preview-toggle]').click();
  await page.setViewportSize({ width: 390, height: 844 });
  await oneRow();
  await page.setViewportSize({ width: 320, height: 844 });
  await oneRow();
  // On very narrow seats, demand measurement collapses the model to its icon.
  await expect.poll(() => model.evaluate(el => getComputedStyle(el.querySelector('span')!).display)).toBe('none');
  await model.click();
  await expect(page.getByRole('menu').first()).toBeVisible();
  await page.keyboard.press('Escape');
  await page.setViewportSize({ width: 390, height: 844 });
  await oneRow();
  await expect.poll(() => model.evaluate(el => getComputedStyle(el.querySelector('span')!).display)).not.toBe('none');
  await page.evaluate(() => document.fonts.dispatchEvent(new Event('loadingdone')));
  await oneRow();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});
