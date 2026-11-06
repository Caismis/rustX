import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose } from './shell-actions';
for (const locale of ['en', 'zh'] as const) {
  test(`temporary question coexists with Agent, closes without cancellation and supports explicit cancel ${locale}`, async ({ page }) => {
    const fixture = await startDogfood('web_auxiliary');
    let passed = false;
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    const copy = (en: string, zh: string) => locale === 'en' ? en : zh;
    try {
      await page.setViewportSize({ width: locale === 'zh' ? 390 : 1440, height: 900 });
      await page.addInitScript(locale => { localStorage.setItem('rustx-locale-v1', 'en'); localStorage.setItem('rustx-appearance-v1', locale === 'en' ? 'light' : 'dark'); }, locale);
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token, fixture.deliveryToken);
      await openEmptySession(page, fixture, 'Workspace A');
      if (locale === 'zh') {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      const primary = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
      await primary.fill('Primary remains running'); await primary.press('Enter'); await fixture.gate('primary');
      const opener = page.getByRole('button', { name: copy('Ask a side question', '临时提问'), exact: true });
      await opener.click();
      const panel = page.getByRole('dialog', { name: copy('Ask a side question', '临时提问'), exact: true });
      const question = panel.getByRole('textbox', { name: copy('Temporary question', '临时问题'), exact: true });
      await question.fill('What has been committed?'); await question.press('Control+Enter'); await fixture.gate('btw');
      await expect(panel.getByRole('button', { name: copy('Ask', '提问'), exact: true })).toBeDisabled();
      await page.keyboard.press('Escape'); await expect(panel).toHaveCount(0); await expect(opener).toBeFocused();
      await fixture.release('btw'); await opener.click();
      await expect(panel.locator('[data-btw-complete="true"]')).toHaveText('Temporary answer: committed snapshot only.');
      await expect(primary).toBeVisible();
      await question.fill('Cancel this side question'); await question.press('Control+Enter'); await fixture.gate('cancel-btw');
      await panel.getByRole('button', { name: copy('Cancel question', '取消此问题'), exact: true }).click();
      await expect(panel.getByRole('status')).toHaveText(copy('Cancelled', '已取消'));
      expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
      await page.screenshot({ path: `/tmp/rustx-476-btw-${locale}.png` });
      await page.keyboard.press('Escape'); await fixture.release('cancel-btw'); await fixture.release('primary');
      await expect(page.getByText('Primary provisional text. Primary complete.', { exact: true })).toBeVisible();
      await expect(page.locator('[data-conversation-scroll]')).not.toContainText('Temporary answer:');
      expect(errors).toEqual([]); passed = true;
    } catch (error) { console.error(fixture.diagnostics()); throw error; } finally { await fixture.stop(passed); }
  });
}
