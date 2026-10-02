import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose, expectSettled, connectionAction } from './shell-actions';

for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`native non-modal compaction and measured context ${locale} ${theme}`, async ({ page }) => {
    const fixture = await startDogfood('web_compaction');
    let passed = false;
    try {
      await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
      await page.setViewportSize({ width: theme === 'dark' ? 390 : 1440, height: 1000 });
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openEmptySession(page, fixture, 'Workspace A');
      await page.getByRole('button', { name: 'Toggle Inspector', exact: true }).click();
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(page.getByText('Last request context unavailable', { exact: true })).toBeVisible();
      await input.fill('/compact'); await input.press('Enter');
      await expect(page.getByText('Compaction failed', { exact: true })).toBeVisible();
      const diagnostic = page.getByText('Compaction details', { exact: true });
      await diagnostic.focus(); await diagnostic.press('Enter');
      await expect(page.getByRole('status').locator('details p')).not.toBeEmpty();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await input.fill('compaction-evidence-435 ' + 'Preserve these historical facts. '.repeat(512));
      await input.press('Enter');
      await expect(page.getByText('Original context ready.', { exact: true })).toBeVisible();
      await expectSettled(page);
      await page.getByRole('button', { name: 'Close Inspector', exact: true }).click();
      await expect(page.getByText('Last request context 25%', { exact: true })).toBeVisible();
      const meter = page.locator('summary').filter({ hasText: 'Last request context 25%' });
      await meter.focus(); await meter.press('Enter');
      await expect(page.getByText('32000 / 128000 input tokens · console-model', { exact: true })).toBeVisible();
      if (locale === 'zh') {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      const message = page.getByRole('textbox', { name: locale === 'zh' ? '消息' : 'Message', exact: true });
      await message.fill('/compact'); await message.press('Enter');
      await fixture.gate('manual-summary');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect(page.getByText(locale === 'zh' ? '正在压缩上下文…' : 'Compacting context…', { exact: true })).toBeVisible();
      await expect(message).toHaveValue('');
      await message.fill('Draft survives native maintenance');
      await expect(message).toBeEditable();
      if (locale === 'en' && theme === 'light') {
        await connectionAction(page, 'Reconnect');
        await expect(message).toBeEditable();
        await expect(message).toHaveValue('Draft survives native maintenance');
        await expect(page.getByText('Compacting context…', { exact: true })).toBeVisible();
      }
      await fixture.release('manual-summary');
      await expect(page.getByText(locale === 'zh' ? '上下文已压缩' : 'Context compacted', { exact: true })).toBeVisible();
      await expect(message).toHaveValue('Draft survives native maintenance');
      await expect(page.getByText(locale === 'zh' ? '上次请求上下文用量不可用' : 'Last request context unavailable', { exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      passed = true;
    } finally { await fixture.stop(passed); }
  });
}
