import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const theme of ['light', 'dark']) for (const width of [1440, 390]) {
  test(`fork messages continue without a source notice ${theme} ${width}`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text()); });
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(theme => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', 'zh'); }, theme);
    await page.goto(`${origin}/test/fixtures/agent.html?mode=fork`);
    await expect(page).toHaveTitle(/rustX/);
    const inherited = page.locator('[data-chat-anchor-key="message:answer-1"]');
    const branch = page.getByText('Continue from this fork boundary.', { exact: true });
    await expect(inherited).toBeVisible(); await expect(branch).toBeVisible();
    expect(await inherited.evaluate((node, text) => {
      const branch = [...document.querySelectorAll('[data-chat-anchor-key]')].find(row => row.textContent?.includes(text));
      return !!branch && !!(node.compareDocumentPosition(branch) & Node.DOCUMENT_POSITION_FOLLOWING);
    }, 'Continue from this fork boundary.')).toBe(true);
    await expect(page.getByRole('group', { name: /分叉点|Fork point/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /查看来源|View source/ })).toHaveCount(0);
    await expect(page.locator('[data-chat-anchor-key="fork-point"]')).toHaveCount(0);
    await branch.scrollIntoViewIfNeeded();
    await page.locator('[data-model-select]').click();
    await expect(page.locator('[data-model-menu]')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    await page.screenshot({ path: `/tmp/rustx-fork-continuous-${theme}-${width}.png` });
    expect(errors).toEqual([]);
  });
}
