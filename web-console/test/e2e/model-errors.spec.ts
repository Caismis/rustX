import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose } from './shell-actions';

for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`persistent native model failures ${locale} ${theme}`, async ({ page }) => {
    const fixture = await startDogfood('web_model_errors');
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const copy = (en: string, zh: string) => locale === 'en' ? en : zh;
    try {
      await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
      await page.setViewportSize({ width: theme === 'dark' ? 390 : 1440, height: 1000 });
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token);
      const sessionId = await openEmptySession(page, fixture, 'Workspace A');
      const inspector = page.getByRole('button', { name: 'Toggle Inspector', exact: true });
      if (await inspector.getAttribute('aria-expanded') === 'true') await inspector.click();
      if (locale === 'zh') {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      const input = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
      await input.fill('Authentication failure'); await input.press('Enter');
      const failures = page.locator('[data-turn-error]');
      await expect(failures).toHaveCount(1);
      await expect(failures.first()).toContainText(copy('API key is invalid', 'API 密钥无效'));
      await expect(failures.first()).toContainText('authentication');
      await input.fill('Invalid response'); await input.press('Enter');
      await fixture.gate('invalid-response');
      await expect(page.getByText('Partial output stays readable.', { exact: true })).toBeVisible();
      await fixture.release('invalid-response');
      await expect(failures).toHaveCount(2);
      await expect(failures.last()).toContainText('malformed chat chunk');
      await expect(failures.last()).toContainText('provider_error');
      await expect(page.getByText('Partial output stays readable.', { exact: true })).toBeVisible();
      await expect(input).toBeEditable();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `/tmp/rustx-model-errors-${locale}-${theme}.png` });
      await input.fill('Continue after failure'); await input.press('Enter');
      await expect(page.getByText('Conversation recovered after model errors.', { exact: true })).toBeVisible();
      await expect(failures).toHaveCount(2);
      await page.reload();
      // Remote credentials are deliberately memory-only. A fresh page reconnects
      // explicitly, then reads the same durable Session rather than retained UI.
      await page.getByRole('button', { name: copy('Settings', '设置'), exact: true }).click();
      const settings = page.getByRole('dialog', { name: copy('Settings', '设置'), exact: true });
      const section = settings.getByRole('button', { name: locale === 'en' ? /^Settings page: / : /^设置页面：/ });
      if (await section.isVisible()) {
        await section.click();
        await page.getByRole('menuitem', { name: copy('Advanced', '高级'), exact: true }).click();
      } else await settings.getByRole('tab', { name: copy('Advanced', '高级'), exact: true }).click();
      await settings.getByRole('button', { name: copy('Connection', '连接'), exact: true }).click();
      await page.getByLabel(copy('Connection mode', '连接模式')).selectOption('remote');
      await page.getByLabel(copy('WebSocket endpoint', 'WebSocket 地址')).fill(fixture.endpoint);
      await page.getByLabel(copy('Transport token', '传输令牌')).fill(fixture.token);
      await settings.getByRole('button', { name: copy('Connect', '连接'), exact: true }).click();
      await expect(page.locator('.connection-status')).toHaveText(copy('Connected', '已连接'));
      await page.getByRole('button', { name: copy('Close Settings', '关闭设置'), exact: true }).click();
      const expand = page.getByRole('button', { name: copy('Expand Sidebar', '打开侧边栏'), exact: true });
      if (await expand.isVisible()) await expand.click();
      await page.locator(`button[data-session-id="${sessionId}"]`).click();
      const collapse = page.getByRole('button', { name: copy('Collapse Sidebar', '收起侧边栏'), exact: true });
      if (theme === 'dark' && await collapse.isVisible()) await collapse.click();
      await expect(failures).toHaveCount(2);
      await expect(failures.last()).toContainText('malformed chat chunk');
      await expect(page.getByText('Conversation recovered after model errors.', { exact: true })).toBeVisible();
      expect(errors).toEqual([]); passed = true;
    } finally { await fixture.stop(passed); }
  });
}
