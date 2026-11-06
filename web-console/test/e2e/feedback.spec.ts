import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const [width, zh] of [[1440, false], [390, true]] as const) {
  test(`DSH connection recovery and transient action feedback ${width}`, async ({ page }) => {
    const fixture = await startDogfood('web_fork_feedback'), wire = await wireProbe(page);
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const text = (en: string, cn: string) => zh ? cn : en;
    const editor = page.getByRole('textbox', { name: text('Message', '消息'), exact: true });
    try {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript(dark => localStorage.setItem('rustx-appearance-v1', dark ? 'dark' : 'light'), zh);
      await routeWorkspaceHost(page, fixture); await page.goto('/'); await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
      if (zh) {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      await editor.fill('Fork source'); await editor.press('Enter');
      await expect(page.getByText('Completed fork answer.', { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: text('Regenerate', '重新生成'), exact: true })).toBeEnabled();
      await editor.fill('Keep this unsent draft');
      const reading = await page.locator('.chat-reading-surface').elementHandle();
      const geometry = await editor.boundingBox();
      wire.loseNext('session/tree');
      await page.getByRole('button', { name: text('Session actions', '会话操作'), exact: true }).click();
      await page.getByRole('menuitem', { name: text('Session tree', '会话树'), exact: true }).click();
      await expect.poll(wire.lost).toBe(1);
      await expect(page.getByRole('dialog')).toHaveCount(0);
      const reconnect = page.getByRole('button', { name: text('Reconnect now', '立即重新连接'), exact: true });
      await expect(reconnect).toBeVisible();
      await expect(page.getByText(text('Connection interrupted', '连接已中断'), { exact: true })).toHaveCount(0);
      await expect(page.getByLabel(text('Session status', '会话状态'), { exact: true })).toHaveCount(0);
      await expect(page.getByText(text('Configuration status unavailable. Retaining the last observation.', '配置状态不可用，保留上次观测结果。'), { exact: true })).toHaveCount(0);
      if (width === 1440) expect(await page.getByRole('button', { name: 'Settings', exact: true }).locator('span').evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
      await expect(editor).toHaveValue('Keep this unsent draft');
      expect(await reading!.evaluate(node => node.isConnected)).toBe(true);
      expect(await editor.boundingBox()).toEqual(geometry);
      await page.screenshot({ path: `/tmp/rustx-feedback-offline-${width}.png` });
      wire.holdNext('initialize'); await reconnect.click(); await expect.poll(wire.held).toBe(1);
      await expect(page.getByRole('button', { name: text('Restart connection attempt', '重新发起连接') })).toBeVisible();
      await page.screenshot({ path: `/tmp/rustx-feedback-connecting-${width}.png` });
      wire.releaseHeld();
      await expect(page.getByRole('status', { name: text('Connected', '已连接'), exact: true })).toBeVisible();
      await expect(editor).toHaveValue('Keep this unsent draft');
      await page.screenshot({ path: `/tmp/rustx-feedback-recovered-${width}.png` });
      await expect(page.getByRole('status', { name: text('Connected', '已连接'), exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: text('Send', '发送'), exact: true })).toBeEnabled();
      await editor.fill('/not-a-command'); await editor.press('Enter');
      const toast = page.locator('[data-toast]');
      await expect(toast).toContainText(text('Unsupported command', '不支持此命令'));
      await expect(page.getByRole('listbox', { name: text('Commands', '命令'), exact: true })).toHaveCount(0);
      await expect(editor).toHaveValue('/not-a-command'); await expect(editor).toBeFocused();
      expect(await editor.boundingBox()).toEqual(geometry);
      const first = await toast.elementHandle(); await editor.press('Enter');
      await expect(toast).toHaveCount(1); expect(await first!.evaluate(node => node.isConnected)).toBe(false);
      await page.screenshot({ path: `/tmp/rustx-feedback-action-${width}.png` });
      await expect(toast).toHaveCount(0, { timeout: 9000 });
      await expect(editor).toHaveValue('/not-a-command');
      expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
      passed = true;
    } finally { await page.close(); await fixture.stop(passed); }
  });
}
