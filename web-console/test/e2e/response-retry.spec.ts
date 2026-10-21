import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const [width, zh] of [[1440, false], [390, true]] as const) {
  test(`message Retry removes the old reply immediately and keeps a resident reading surface ${width}`, async ({ page }) => {
    const fixture = await startDogfood('web_response_retry'), wire = await wireProbe(page);
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const message = page.getByRole('textbox', { name: zh ? '消息' : 'Message', exact: true });
    const settled = async () => {
      // The narrow sidebar is unmounted. Read the exact native attached target,
      // independently of whether a sidebar Session button is in the DOM.
      const target = [...wire.responses].reverse().find(row => row.method === 'session/attach')?.result?.target;
      expect(target).toBeTruthy();
      await expect.poll(async () => (await wire.read('session/checkpoint', target)).checkpoint.snapshot.attempt?.phase.type).toBe('settled');
    };
    const send = async (text: string) => { await message.fill(text); await page.getByRole('button', { name: zh ? '发送' : 'Send', exact: true }).click(); };
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
      await send('Earlier question'); await expect(page.getByText('Earlier answer stays.', { exact: true })).toBeVisible(); await settled();
      await send('Retry this question'); await expect(page.getByRole('heading', { name: 'Original response' })).toBeVisible(); await settled();
      const surface = await page.locator('.chat-reading-surface').elementHandle(), editor = await message.elementHandle();
      wire.holdNext('session/branch');
      await page.getByRole('button', { name: zh ? '重新生成' : 'Regenerate', exact: true }).last().click();
      await expect.poll(wire.held).toBe(1);
      await expect(page.getByRole('heading', { name: 'Original response' })).toHaveCount(0);
      await expect(page.getByText('Retry this question', { exact: true })).toHaveCount(1);
      await expect(page.locator('[data-chat-running]')).toHaveCount(1);
      await expect(page.getByLabel(zh ? '会话状态' : 'Session status', { exact: true })).toContainText(zh ? '正在准备重新生成…' : 'Preparing a new response…');
      await expect(page.getByText(zh ? '连接已中断' : 'Connection interrupted', { exact: true })).toHaveCount(0);
      await expect(page.getByText(zh ? '准备好接受任务' : 'Ready for a task', { exact: true })).toHaveCount(0);
      await page.evaluate(() => {
        const frames: { old: boolean; input: boolean; previous: boolean; surface: boolean; editor: boolean; disconnected: boolean }[] = [];
        const surface = document.querySelector('.chat-reading-surface'), editor = document.querySelector('[data-resident-composer] textarea');
        const state = { active: true, frames };
        (window as any).__retryProbe = state;
        const sample = () => {
          const text = surface?.textContent ?? '';
          frames.push({ old: text.includes('Old reply to replace.'), input: text.includes('Retry this question'), previous: text.includes('Earlier answer stays.'), surface: !!surface?.isConnected, editor: !!editor?.isConnected, disconnected: /Connection interrupted|连接已中断/.test(document.body.textContent ?? '') });
          if (state.active) requestAnimationFrame(sample);
        };
        requestAnimationFrame(sample);
      });
      await page.screenshot({ path: `/tmp/rustx-retry-pending-${width}.png` });
      // Hold real acknowledgements at every native handoff; no semantic result is fabricated.
      for (const next of ['session/switchNode', 'session/attach', 'turn/start']) {
        wire.holdNext(next); wire.releaseHeld(); await expect.poll(wire.held).toBe(1);
        await expect(page.getByRole('heading', { name: 'Original response' })).toHaveCount(0);
        await expect(page.getByText('Retry this question', { exact: true })).toHaveCount(1);
        await expect(page.getByLabel(zh ? '会话状态' : 'Session status', { exact: true })).toContainText(zh ? '正在准备重新生成…' : 'Preparing a new response…');
        await expect(page.getByText(zh ? '连接已中断' : 'Connection interrupted', { exact: true })).toHaveCount(0);
        expect(await surface!.evaluate(node => node.isConnected)).toBe(true);
        expect(await editor!.evaluate(node => node.isConnected)).toBe(true);
        if (next === 'session/switchNode') await page.screenshot({ path: `/tmp/rustx-retry-handoff-${width}.png` });
      }
      await fixture.gate('replay-request'); wire.releaseHeld();
      await expect(page.locator('[data-chat-running]')).toHaveCount(1);
      await fixture.release('replay-request'); await fixture.gate('replay-streaming');
      await expect(page.getByRole('heading', { name: 'New response' })).toBeVisible();
      await expect(page.getByText('Generating the replacement.', { exact: true })).toBeVisible();
      await page.screenshot({ path: `/tmp/rustx-retry-streaming-${width}.png` });
      await fixture.release('replay-streaming'); await expect(page.getByText('Replacement complete.', { exact: true })).toBeVisible(); await settled();
      await expect(page.locator('[data-chat-running]')).toHaveCount(0);
      await expect(page.getByRole('heading', { name: 'Original response' })).toHaveCount(0);
      await expect(page.getByText('Retry this question', { exact: true })).toHaveCount(1);
      const frames = await page.evaluate(() => { const state = (window as any).__retryProbe; state.active = false; return state.frames as {old: boolean; input: boolean; previous: boolean; surface: boolean; editor: boolean; disconnected: boolean}[]; });
      expect(frames.length).toBeGreaterThan(0);
      expect(frames.every(frame => !frame.old && frame.input && frame.previous && frame.surface && frame.editor && !frame.disconnected)).toBe(true);
      expect(wire.requests.filter(request => request.method === 'session/branch')).toHaveLength(1);
      expect(wire.requests.filter(request => request.method === 'turn/start')).toHaveLength(3);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
      passed = true;
    } finally { await page.close(); await fixture.stop(passed); }
  });
}
