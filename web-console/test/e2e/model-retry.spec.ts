import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose } from './shell-actions';

for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`native timeout retry disclosure ${locale} ${theme}`, async ({ page }) => {
    const fixture = await startDogfood('web_timeout_retry');
    let passed = false;
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    const copy = (en: string, zh: string) => locale === 'en' ? en : zh;
    try {
      await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
      await page.setViewportSize({ width: theme === 'dark' ? 390 : 1440, height: 1000 });
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
      const inspector = page.getByRole('button', { name: 'Toggle Inspector', exact: true });
      if (await inspector.getAttribute('aria-expanded') === 'true') await inspector.click();
      if (locale === 'zh') {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      const input = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
      await input.fill('Timeout and recover'); await input.press('Enter'); await fixture.gate('retry-started');
      const notices = page.locator('[data-model-retry]');
      await expect(notices).toHaveCount(1);
      await expect(notices.first().locator('summary')).toContainText(copy('Retrying model request (1)', '正在重试模型请求（1）'));
      await expect(notices.first()).toHaveAttribute('data-active', 'true');
      await notices.first().locator('summary').click();
      await expect(notices.first()).toContainText('Fixture model response timeout');
      await expect(page.getByRole('button', { name: copy('Stop', '停止'), exact: true })).toBeEnabled();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `/tmp/rustx-timeout-retry-${locale}-${theme}.png` });
      await fixture.release('retry-started');
      await expect(page.getByText('Recovered after timeout.', { exact: true })).toBeVisible();
      await expect(notices).toHaveCount(1);
      await expect(notices.first().locator('summary')).toContainText(copy('Model request retried (1)', '已重试模型请求（1）'));
      await expect(page.locator('[data-model-retry][data-active]')).toHaveCount(0);
      await input.fill('Stop during retry'); await input.press('Enter'); await fixture.gate('stop-retry');
      await expect(page.locator('[data-model-retry][data-active]')).toHaveCount(1);
      await page.getByRole('button', { name: copy('Stop', '停止'), exact: true }).click();
      await expect(page.getByRole('button', { name: copy('Send', '发送'), exact: true })).toBeVisible();
      await expect(page.locator('[data-model-retry][data-active]')).toHaveCount(0);
      await fixture.release('stop-retry');
      await expect(page.getByText('Late retry output must stay hidden.', { exact: true })).toHaveCount(0);
      await expect(input).toBeEditable();
      expect(errors).toEqual([]); passed = true;
    } finally { await fixture.stop(passed); }
  });
}
