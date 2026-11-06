import { test, expect } from '@playwright/test';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const locale of ['en', 'zh'] as const) for (const narrow of [false, true]) {
  test(`Harness tab menu closes only its target and dismisses cleanly (${locale}, ${narrow ? 'narrow' : 'desktop'})`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize(narrow ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
    await page.addInitScript(language => localStorage.setItem('rustx-locale-v1', language), locale);
    await page.emulateMedia({ colorScheme: narrow ? 'light' : 'dark' });
    await page.goto(`${origin}/test/fixtures/workbench.html`);
    await expect(page).toHaveTitle('Session A — rustX');
    await page.locator('[data-preview-toggle]').click();
    const panel = page.locator('[data-workbench]');
    await panel.getByRole('button', { name: locale === 'en' ? /Workspace files/ : /工作区文件/ }).click();
    await panel.getByRole('button', { name: 'docs', exact: true }).click();
    await panel.getByRole('button', { name: 'guide.md', exact: true }).click();
    const file = panel.getByRole('tab', { name: /guide.md/ });
    const files = panel.getByRole('tab', { name: locale === 'en' ? /Files/ : /文件/ });
    await files.click();
    await file.click({ button: 'right' });
    const menu = page.locator('[data-dockkit-tab-menu]');
    const close = menu.getByRole('menuitem', { name: locale === 'en' ? 'Close' : '关闭', exact: true });
    await expect(menu).toBeVisible();
    await expect(menu.getByRole('menuitem')).toHaveCount(1);
    await expect(files).toHaveAttribute('aria-selected', 'true');
    await expect(panel.locator('[data-dockkit-float]')).toHaveCount(0);
    expect(await menu.evaluate(element => {
      const style = getComputedStyle(element), box = element.getBoundingClientRect();
      return { padding: style.padding, minWidth: style.minWidth,
        matchesRadius: style.borderRadius === style.getPropertyValue('--dsw-radius-lg').trim(),
        withinViewport: box.left >= 0 && box.right <= innerWidth };
    })).toEqual({ padding: '4px', minWidth: '96px', matchesRadius: true, withinViewport: true });
    await close.hover();
    await page.screenshot({ path: info.outputPath('tab-context-menu.png') });

    await close.focus();
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);
    await expect(file).toBeFocused();
    await expect(files).toHaveAttribute('aria-selected', 'true');
    await file.click({ button: 'right' });
    await expect(menu).toBeVisible();
    await files.click();
    await expect(menu).toHaveCount(0);

    await file.click({ button: 'right' });
    await close.click();
    await expect(file).toHaveCount(0);
    await expect(files).toHaveAttribute('aria-selected', 'true');
    await files.click({ button: 'right' });
    await close.click();
    await expect(page.locator('[data-sidebar-right-open]')).toHaveCount(0);
  await expect(panel.locator('[data-dockkit-host="dock"]:visible')).toHaveCount(0);
    await page.locator('[data-preview-toggle]').click();
    await panel.getByRole('tab', { name: locale === 'en' ? 'Start' : '开始', exact: true }).click({ button: 'right' });
    await expect(menu).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
