import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const [width, zh] of [[1440, false], [390, true]] as const) {
  test(`message Fork responds immediately and opens before settings details ${width}`, async ({ page }) => {
    const fixture = await startDogfood('web_fork_feedback'), wire = await wireProbe(page);
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const message = page.getByRole('textbox', { name: zh ? '消息' : 'Message', exact: true });
    const fork = page.getByRole('button', { name: zh ? '在新会话中分支' : 'Branch into a new Session', exact: true });
    const status = page.getByLabel(zh ? '会话状态' : 'Session status', { exact: true });
    try {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript(dark => localStorage.setItem('rustx-appearance-v1', dark ? 'dark' : 'light'), zh);
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
      if (zh) {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      await message.fill('Fork source'); await page.getByRole('button', { name: zh ? '发送' : 'Send', exact: true }).click();
      await expect(page.getByText('Completed fork answer.', { exact: true })).toBeVisible();
      // Native settled/admission evidence enables Retry; observe that same UI
      // proof instead of adding competing checkpoint reads to the socket.
      await expect(page.getByRole('button', { name: zh ? '重新生成' : 'Regenerate', exact: true })).toBeEnabled();
      const surface = await page.locator('.chat-reading-surface').elementHandle();
      wire.holdNext('session/fork'); await fork.click(); await expect.poll(wire.held).toBe(1);
      await expect(fork).toBeDisabled(); await expect(fork).toHaveAttribute('aria-busy', 'true');
      await expect(fork.locator('[data-state=ongoing]')).toBeVisible();
      await expect(status).toContainText(zh ? '正在分叉会话…' : 'Forking conversation…');
      await page.screenshot({ path: `/tmp/rustx-message-fork-pending-${width}.png` });
      for (const next of ['session/attach', 'session/subscribe']) {
        wire.holdNext(next); wire.releaseHeld(); await expect.poll(wire.held).toBe(1);
        await expect(fork).toHaveAttribute('aria-busy', 'true');
        await expect(page.getByText('Completed fork answer.', { exact: true })).toBeVisible();
        expect(await surface!.evaluate(node => node.isConnected)).toBe(true);
      }
      // This settings read follows the native checkpoint/subscription. The
      // child must already be open while its real acknowledgement is held.
      wire.holdNext('session/settings'); wire.releaseHeld(); await expect.poll(wire.held).toBe(1);
      const child = wire.responses.find(row => row.method === 'session/fork')!.result.session.id;
      await expect(fork).not.toHaveAttribute('aria-busy', 'true');
      await expect(status).toHaveCount(0);
      if (width === 390) await page.getByRole('button', { name: '打开侧边栏', exact: true }).click();
      await expect(page.locator(`button[data-session-id="${child}"]`)).toHaveAttribute('aria-current', 'page');
      if (width === 390) await page.getByRole('button', { name: '关闭导航', exact: true }).click({ position: { x: width - 20, y: 400 } });
      await expect(page.getByText('Completed fork answer.', { exact: true })).toBeVisible();
      await expect(page.locator('.chat-reading-surface').getByText('Fork source', { exact: true })).toHaveCount(1);
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await page.screenshot({ path: `/tmp/rustx-message-fork-ready-${width}.png` });
      wire.releaseHeld();
      expect(wire.requests.filter(row => row.method === 'session/fork')).toHaveLength(1);
      expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
      passed = true;
    } finally { await page.close(); await fixture.stop(passed); }
  });
}
