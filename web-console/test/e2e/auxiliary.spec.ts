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
      const opener = page.locator('[data-side-question-toggle]');
      await opener.click();
      const panel = page.getByRole('form', { name: copy('Ask a side question', '临时提问'), exact: true });
      await expect(page.getByRole('dialog', { name: copy('Ask a side question', '临时提问'), exact: true })).toHaveCount(0);
      const question = panel.getByRole('textbox', { name: copy('Temporary question', '临时问题'), exact: true });
      await expect(question).toBeFocused(); await question.fill('What has been committed?');
      await question.press('Escape'); await expect(panel).toBeHidden(); await expect(opener).toBeFocused();
      await opener.click(); await expect(question).toHaveValue('What has been committed?');
      await question.press('Control+Enter'); await fixture.gate('btw');
      await expect(panel.getByRole('button', { name: copy('Cancel question', '取消此问题'), exact: true })).toBeEnabled();
      await expect(panel.locator('button[type="submit"]')).toHaveCount(0);
      await panel.focus(); await page.keyboard.press('Escape'); await expect(panel).toBeHidden(); await expect(opener).toBeFocused();
      await opener.click(); await expect(question).toHaveValue(''); await expect(question).toBeDisabled();
      await expect(panel.getByRole('article', { name: copy('Your message', '你的消息'), exact: true })).toHaveText('What has been committed?');
      await panel.focus(); await page.keyboard.press('Escape');
      await fixture.release('btw'); await opener.click();
      await expect(panel.locator('[data-btw-complete="true"]')).toHaveText('Temporary answer: committed snapshot only.');
      await expect(question).toHaveValue('');
      await expect(primary).toBeVisible();
      await page.screenshot({ path: `/tmp/rustx-pr517-btw-chat-${locale}.png` });
      await page.setViewportSize({ width: locale === 'zh' ? 390 : 1440, height: 480 });
      const composer = panel.locator('[data-btw-composer]');
      await expect(composer).toBeVisible(); await expect(question).toBeVisible();
      const panelBox = (await panel.boundingBox())!, composerBox = (await composer.boundingBox())!;
      expect(composerBox.y).toBeGreaterThanOrEqual(panelBox.y); expect(composerBox.y + composerBox.height).toBeLessThanOrEqual(panelBox.y + panelBox.height + 1);
      await question.fill('Cancel this side question'); await question.press('Control+Enter'); await fixture.gate('cancel-btw');
      const cancel = panel.getByRole('button', { name: copy('Cancel question', '取消此问题'), exact: true });
      await expect(cancel).toBeVisible(); await cancel.click();
      await expect(panel.getByRole('status')).toHaveText(copy('Cancelled', '已取消'));
      expect(await panel.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
      await page.screenshot({ path: `/tmp/rustx-476-btw-${locale}.png` });
      await page.keyboard.press('Escape'); await fixture.release('cancel-btw'); await fixture.release('primary');
      await expect(page.getByText('Primary provisional text. Primary complete.', { exact: true })).toBeVisible();
      await expect(page.locator('[data-conversation-scroll]')).not.toContainText('Temporary answer:');
      expect(errors).toEqual([]); passed = true;
    } catch (error) { console.error(fixture.diagnostics()); throw error; } finally {
      // Retire browser observers and proxy requests while their fixture Host
      // is still live, then settle the Host before its native process exits.
      try { await page.close(); } finally { await fixture.stop(passed); }
    }
  });
}
