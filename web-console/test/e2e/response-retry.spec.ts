import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import type { CompletedResponseView } from '../../../protocol/app-server/v57';

for (const [width, zh] of [[1440, false], [390, true]] as const) {
  test(`message Retry removes the old reply immediately and keeps a resident reading surface ${width}`, async ({ page }) => {
    const fixture = await startDogfood('web_response_retry'), wire = await wireProbe(page);
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const message = page.getByRole('textbox', { name: zh ? '消息' : 'Message', exact: true });
    const settled = async (previousAttempt?: string) => {
      // The narrow sidebar is unmounted. Read the exact native attached target,
      // independently of whether a sidebar Session button is in the DOM.
      const target = [...wire.responses].reverse().find(row => row.method === 'session/attach')?.result?.target;
      expect(target).toBeTruthy();
      let attemptId: string | undefined, response: CompletedResponseView | undefined;
      await expect.poll(async () => {
        const checkpoint = (await wire.read('session/checkpoint', target)).checkpoint;
        const attempt = checkpoint.snapshot.attempt;
        if (!attempt?.attempt_id || attempt.attempt_id === previousAttempt || attempt.phase.type !== 'settled') return false;
        attemptId = attempt.attempt_id;
        response = checkpoint.snapshot.transcript.entries?.find(entry =>
          entry.completed_response?.origin.conversation_id === target.conversation_id
          && entry.completed_response?.origin.attempt_id === attemptId)?.completed_response ?? undefined;
        return !!response;
      }).toBe(true);
      if (!attemptId || !response) throw new Error('The settled response lacks its native action coordinates.');
      return { attemptId, response };
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
      await send('Earlier question'); await expect(page.getByText('Earlier answer stays.', { exact: true })).toBeVisible(); const earlier = await settled();
      await send('Retry this question'); await expect(page.getByRole('heading', { name: 'Original response' })).toBeVisible(); const original = await settled(earlier.attemptId);
      const surface = await page.locator('.chat-reading-surface').elementHandle(), editor = await message.elementHandle();
      // Settlement and the response action tail have separate publication frontiers.
      const retry = page.locator(`[data-turn-tail=${JSON.stringify(attemptOwnerKey(original.response.origin))}]`)
        .getByRole('button', { name: zh ? '重新生成' : 'Regenerate', exact: true });
      expect(original.response.retry_locator).toBeTruthy();
      await expect(retry).toBeVisible();
      wire.holdNext('session/branch');
      await retry.click();
      await expect.poll(wire.held).toBe(1);
      const branches = wire.requests.filter(request => request.method === 'session/branch');
      expect(branches).toHaveLength(1);
      expect(branches[0].params).toMatchObject({
        boundary: original.response.retry_locator,
        surface_revision: original.response.surface_revision,
        side: 'before',
      });
      await expect(page.getByRole('heading', { name: 'Original response' })).toHaveCount(0);
      await expect(page.getByText('Retry this question', { exact: true })).toHaveCount(1);
      await expect(page.locator('[data-chat-running]')).toHaveCount(1);
      await expect(page.getByText(zh ? '准备好接受任务' : 'Ready for a task', { exact: true })).toHaveCount(0);
      await page.evaluate(() => {
        const frames: { old: boolean; input: boolean; previous: boolean; surface: boolean; editor: boolean }[] = [];
        const surface = document.querySelector('.chat-reading-surface'), editor = document.querySelector('[data-resident-composer] textarea');
        const state = { active: true, frames };
        (window as any).__retryProbe = state;
        const sample = () => {
          const text = surface?.textContent ?? '';
          frames.push({ old: text.includes('Old reply to replace.'), input: text.includes('Retry this question'), previous: text.includes('Earlier answer stays.'), surface: !!surface?.isConnected, editor: !!editor?.isConnected });
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
        expect(await surface!.evaluate(node => node.isConnected)).toBe(true);
        expect(await editor!.evaluate(node => node.isConnected)).toBe(true);
      }
      await fixture.gate('replay-request'); wire.releaseHeld();
      await expect(page.locator('[data-chat-running]')).toHaveCount(1);
      await fixture.release('replay-request'); await fixture.gate('replay-streaming');
      await expect(page.getByRole('heading', { name: 'New response' })).toBeVisible();
      await expect(page.getByText('Generating the replacement.', { exact: true })).toBeVisible();
      await page.screenshot({ path: `/tmp/rustx-retry-streaming-${width}.png` });
      await fixture.release('replay-streaming'); await expect(page.getByText('Replacement complete.', { exact: true })).toBeVisible(); await settled(original.attemptId);
      await expect(page.locator('[data-chat-running]')).toHaveCount(0);
      await expect(page.getByRole('heading', { name: 'Original response' })).toHaveCount(0);
      await expect(page.getByText('Retry this question', { exact: true })).toHaveCount(1);
      const frames = await page.evaluate(() => { const state = (window as any).__retryProbe; state.active = false; return state.frames as {old: boolean; input: boolean; previous: boolean; surface: boolean; editor: boolean}[]; });
      expect(frames.length).toBeGreaterThan(0);
      expect(frames.every(frame => !frame.old && frame.input && frame.previous && frame.surface && frame.editor)).toBe(true);
      expect(wire.requests.filter(request => request.method === 'session/branch')).toHaveLength(1);
      expect(wire.requests.filter(request => request.method === 'turn/start')).toHaveLength(3);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
      passed = true;
    } finally { await page.close(); await fixture.stop(passed); }
  });
}
