import { test, expect, type Page } from '@playwright/test';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
const panelOf = (page: Page) => page.locator('[data-workbench]');
async function openFiles(page: Page) {
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await page.locator('[data-preview-toggle]').click();
  await panelOf(page).getByRole('button', { name: /Workspace files/ }).click();
  await panelOf(page).getByRole('button', { name: 'docs', exact: true }).click();
}
async function floatTab(page: Page, name: RegExp) {
  const chip = panelOf(page).getByRole('tab', { name });
  await chip.focus();
  await chip.hover();
  const box = (await chip.boundingBox())!;
  await page.mouse.move(box.x + 25, box.y + 14);
  await page.mouse.down();
  await page.mouse.move(480, 200, { steps: 12 });
  await page.mouse.up();
  await expect(panelOf(page).locator('[data-dockkit-float]')).toHaveCount(1);
}

for (const locale of ['en', 'zh'] as const) for (const narrow of [false, true]) {
  test(`Harness chrome and split explanations (${locale}, ${narrow ? 'narrow' : 'desktop'})`, async ({ page }, info) => {
    await page.addInitScript(language => localStorage.setItem('rustx-locale-v1', language), locale);
    await page.setViewportSize(narrow ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
    await page.emulateMedia({ colorScheme: narrow ? 'light' : 'dark' });
    await page.goto(`${origin}/test/fixtures/workbench.html`);
    await page.locator('[data-preview-toggle]').click();
    const panel = panelOf(page), mode = panel.locator('[data-sidebar-right-mode]');
    const collapse = panel.getByRole('button', { name: locale === 'zh' ? '收起右侧边栏' : 'Collapse right sidebar', exact: true });
    const boxes = await panel.locator('[data-dockkit-split-button], [data-sidebar-right-mode], [data-sidebar-right-toggle]').evaluateAll(elements => elements.map(element => {
      const box = element.getBoundingClientRect();
      return { left: box.left, right: box.right, width: box.width, height: box.height, radius: getComputedStyle(element).borderRadius };
    }));
    expect(boxes.map(box => [box.width, box.height, box.radius])).toEqual([[28, 28, '8px'], [28, 28, '8px'], [28, 28, '8px']]);
    expect(boxes[1]!.left - boxes[0]!.right).toBeCloseTo(8, 3);
    expect(boxes[2]!.left - boxes[1]!.right).toBeCloseTo(8, 3);
    await collapse.hover();
    await expect(panel.getByRole('tooltip', { name: locale === 'zh' ? '收起侧边栏' : 'Collapse sidebar', exact: true })).toBeVisible();
    await mode.hover();
    await expect(panel.getByRole('tooltip')).toContainText(narrow ? (locale === 'zh' ? '退出全屏' : 'Exit fullscreen') : (locale === 'zh' ? '全屏' : 'Fullscreen'));
    await expect(panel.getByRole('tooltip').locator('kbd')).toHaveCount(5);
    const split = panel.locator('[data-dockkit-split-button]');
    if (narrow) {
      await expect(split).toBeDisabled();
      await split.locator('..').hover();
      await expect(panel.getByRole('tooltip')).toContainText(locale === 'zh' ? '栏宽不足' : 'Not enough width');
    } else {
      await split.click();
      await panel.locator('[data-dockkit-split-button]').last().locator('..').hover();
      await expect(panel.getByRole('tooltip')).toContainText(locale === 'zh' ? '已达两格上限' : 'Two panes is the limit');
    }
    await page.screenshot({ path: info.outputPath('right-sidebar-chrome.png') });
  });
}

test('page shortcuts respect focus, menus and composition, and return focus after split and close', async ({ page }) => {
  await openFiles(page);
  const panel = panelOf(page);
  await page.getByRole('textbox', { name: 'Message', exact: true }).focus();
  await page.keyboard.press('Control+Backslash');
  await expect(panel.locator('[data-dockkit-pane]')).toHaveCount(1);
  await panel.locator('[data-dockkit-pane]').focus();
  await panel.dispatchEvent('compositionstart');
  await page.keyboard.press('Control+Backslash');
  await expect(panel.locator('[data-dockkit-pane]')).toHaveCount(1);
  await panel.dispatchEvent('compositionend');
  await page.keyboard.press('a');
  await page.keyboard.press('Control+Backslash');
  await expect(panel.locator('[data-dockkit-pane]')).toHaveCount(2);
  await expect(panel.locator('[data-dockkit-pane]').last()).toBeFocused();
  await page.keyboard.press('Control+Alt+Enter');
  await expect(panel.locator('[data-sidebar-right-mode]')).toHaveAttribute('data-sidebar-right-mode', 'push');
  await page.keyboard.press('Control+Alt+W');
  await expect(panel.locator('[data-dockkit-pane]')).toHaveCount(1);
  await expect(panel.locator('[data-dockkit-pane]')).toBeFocused();
  const files = panel.getByRole('tab', { name: /Files/ });
  await files.click({ button: 'right' });
  await page.keyboard.press('Control+Alt+W');
  await expect(files).toHaveCount(1);
  await page.keyboard.press('Escape');
});

test('collapsing and floating retain the selected HTML document and its live state', async ({ page }) => {
  await page.addInitScript(() => { if (window === window.top) localStorage.setItem('rustx-html-interactive-v1', 'true'); });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await openFiles(page);
  const panel = panelOf(page);
  await panel.getByRole('button', { name: 'interactive.html', exact: true }).click();
  await panel.frameLocator('iframe').getByRole('button', { name: 'Count 0' }).click();
  await panel.getByRole('button', { name: 'Collapse right sidebar', exact: true }).click();
  await expect(page.locator('[data-sidebar-right-open]')).toHaveCount(0);
  await expect(panel.locator('iframe')).toHaveCount(1);
  await page.locator('[data-preview-toggle]').click();
  await expect(panel.frameLocator('iframe').getByRole('button', { name: 'Count 1' })).toBeVisible();
  await floatTab(page, /interactive.html/);
  const floating = panel.locator('[data-dockkit-float]');
  await expect(floating.frameLocator('iframe').getByRole('button', { name: 'Count 1' })).toBeVisible();
  await expect(floating).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(floating).toBeVisible();
  await floating.getByRole('button', { name: 'Send back to the sidebar', exact: true }).click();
  await expect(floating).toHaveCount(0);
  await expect(panel.locator('[data-dockkit-pane]')).toBeFocused();
  await expect(panel.frameLocator('iframe').getByRole('button', { name: 'Count 1' })).toBeVisible();
  expect(errors).toEqual([]);
});

test('closing the last docked page collapses and resets fullscreen while leaving floats usable', async ({ page }) => {
  await openFiles(page);
  const panel = panelOf(page);
  await panel.getByRole('button', { name: 'guide.md', exact: true }).click();
  await floatTab(page, /guide.md/);
  await panel.getByRole('button', { name: 'Fullscreen', exact: true }).click();
  await panel.getByRole('tab', { name: /Files/ }).getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.locator('[data-sidebar-right-open]')).toHaveCount(0);
  const floating = panel.locator('[data-dockkit-float]');
  await expect(floating.getByRole('heading', { name: 'Rendered document' })).toBeVisible();
  await page.locator('[data-preview-toggle]').click();
  await expect(panel.getByRole('button', { name: 'Fullscreen', exact: true })).toBeVisible();
  const start = panel.getByRole('tab', { name: 'Start', exact: true });
  await start.click({ button: 'right' });
  await expect(page.locator('[data-dockkit-tab-menu]')).toHaveCount(0);
  await expect(start.getByRole('button', { name: 'Close', exact: true })).toHaveCount(0);
  await floating.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(floating).toHaveCount(0);
  await expect(panel.locator('[data-dockkit-pane]')).toBeFocused();
});

test('a floated Start page merges into the existing docked Start page when sent back', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await page.locator('[data-preview-toggle]').click();
  const panel = panelOf(page);
  await floatTab(page, /Start/);
  await expect(panel.getByRole('tab', { name: 'Start', exact: true })).toHaveCount(1);
  await panel.locator('[data-dockkit-float-dock]').click();
  await expect(panel.locator('[data-dockkit-float]')).toHaveCount(0);
  await expect(panel.getByRole('tab', { name: 'Start', exact: true })).toHaveCount(1);
});

test('long tab titles use Harness fading and keyboard navigation keeps the active chip in view', async ({ page }) => {
  await openFiles(page);
  const panel = panelOf(page);
  for (const name of ['guide.md', 'interactive.html', 'モルガン_解説.md']) {
    await panel.getByRole('tab', { name: /Files/ }).click();
    await panel.getByRole('button', { name, exact: true }).click();
  }
  await page.setViewportSize({ width: 390, height: 844 });
  const title = panel.getByRole('tab', { name: /モルガン_解説.md/ }).locator('[data-dockkit-tab-title]');
  await expect(title).toHaveAttribute('data-dockkit-tab-clipped', '');
  expect(await title.locator('span span').evaluate(element => getComputedStyle(element).textOverflow)).toBe('clip');
  const chip = panel.getByRole('tab', { name: /モルガン_解説.md/ });
  await chip.focus();
  await page.keyboard.press('Home');
  await page.keyboard.press('Enter');
  await expect(panel.getByRole('tab', { name: /Files/ })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('End');
  await page.keyboard.press('Enter');
  await expect(chip).toHaveAttribute('aria-selected', 'true');
  await expect.poll(async () => chip.evaluate(element => {
    const strip=element.closest('[data-dockkit-strip-tabs]')!.getBoundingClientRect(), box=element.getBoundingClientRect();
    return box.left >= strip.left - 1 && box.right <= strip.right + 1;
  })).toBe(true);
});

test('a collapsed draft sidebar keeps workspace availability notices off the conversation', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/workbench.html?draft`);
  const notice = page.getByText('Select a local workspace to use workspace tools.', { exact: true });
  await expect(notice).toHaveCount(0);
  await page.getByRole('button', { name: 'Select Workspace Workspace A', exact: true }).click();
  await page.locator('[data-preview-toggle]').click();
  await expect(panelOf(page).getByRole('button', { name: /Workspace files/ })).toBeEnabled();
  await panelOf(page).getByRole('button', { name: 'Collapse right sidebar', exact: true }).click();
  await expect(notice).toHaveCount(0);
});

test('floating panels move and resize, and pointer cancellation discards an unfinished drag', async ({ page }, info) => {
  await openFiles(page);
  const panel = panelOf(page);
  await panel.getByRole('button', { name: 'guide.md', exact: true }).click();
  await floatTab(page, /guide.md/);
  const floating = panel.locator('[data-dockkit-float]');
  const before = (await floating.boundingBox())!;
  const corner = (await floating.locator('[data-dockkit-float-resize]').boundingBox())!;
  await page.mouse.move(corner.x + corner.width / 2, corner.y + corner.height / 2);
  await page.mouse.down();
  await page.mouse.move(corner.x + corner.width / 2 + 90, corner.y + corner.height / 2 + 60, { steps: 10 });
  await page.mouse.up();
  await expect.poll(async () => Math.round((await floating.boundingBox())!.width - before.width)).toBe(90);
  expect(Math.round((await floating.boundingBox())!.height - before.height)).toBe(60);
  const saved = (await floating.boundingBox())!;
  const grip = floating.locator('[data-dockkit-float-grip]');
  const header = (await grip.boundingBox())!;
  await page.mouse.move(header.x + 70, header.y + 15);
  await page.mouse.down();
  await page.mouse.move(header.x + 150, header.y + 55, { steps: 10 });
  await expect.poll(async () => Math.round((await floating.boundingBox())!.x - saved.x)).toBe(80);
  const pointerId = Number(await grip.getAttribute('data-dockkit-pointer'));
  await grip.dispatchEvent('pointercancel', { pointerId, bubbles: true });
  await page.mouse.up();
  await expect.poll(async () => Math.round((await floating.boundingBox())!.x - saved.x)).toBe(0);
  expect(Math.round((await floating.boundingBox())!.y - saved.y)).toBe(0);
  await page.mouse.move(header.x + 70, header.y + 15);
  await page.mouse.down();
  await page.mouse.move(header.x + 130, header.y + 45, { steps: 10 });
  await page.mouse.up();
  await expect.poll(async () => Math.round((await floating.boundingBox())!.x - saved.x)).toBe(60);
  await page.screenshot({ path: info.outputPath('float-move-resize.png') });
});
