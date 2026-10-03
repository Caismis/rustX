import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose, connectionAction } from './shell-actions';

for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`native non-modal compaction and measured context ${locale} ${theme}`, async ({ page }) => {
    const fixture = await startDogfood('web_compaction');
    let passed = false;
    let holdCompact = false;
    let releaseCompact: (() => void) | undefined;
    await page.routeWebSocket(/ws:\/\//, socket => {
      const server = socket.connectToServer();
      socket.onMessage(message => {
        if (holdCompact && JSON.parse(String(message)).method === 'context/compact') {
          holdCompact = false; releaseCompact = () => server.send(message);
        } else server.send(message);
      });
      server.onMessage(message => socket.send(message));
    });
    const copy = (en: string, zh: string) => locale === 'zh' ? zh : en;
    try {
      await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
      await page.setViewportSize({ width: theme === 'dark' ? 390 : 1440, height: 1000 });
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openEmptySession(page, fixture, 'Workspace A');
      const inspector = page.getByRole('button', { name: 'Toggle Inspector', exact: true });
      if (await inspector.getAttribute('aria-expanded') === 'true') await inspector.click();
      if (locale === 'zh') {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      const input = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
      await expect(page.getByText(copy('Last request context unavailable', '上次请求上下文用量不可用'), { exact: true })).toBeVisible();
      await input.fill('/compact'); await input.press('Enter');
      await expect(page.getByText(copy('Compaction failed', '上下文压缩失败'), { exact: true })).toBeVisible();
      const diagnostic = page.getByText(copy('Compaction details', '上下文压缩详情'), { exact: true });
      await diagnostic.focus(); await diagnostic.press('Enter');
      await expect(page.getByRole('status').locator('details p')).not.toBeEmpty();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await page.getByRole('button', { name: copy('Read current context', '读取当前上下文'), exact: true }).click();
      await expect(page.getByText(copy('Compaction failed', '上下文压缩失败'), { exact: true })).toBeVisible();
      await input.fill('compaction-evidence-435 ' + 'Preserve these historical facts. '.repeat(512));
      await input.press('Enter');
      await expect(page.getByText('Original context ready.', { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: copy('Send', '发送'), exact: true })).toBeVisible();
      await expect(page.getByText(copy('Last request context 25%', '上次请求上下文用量 25%'), { exact: true })).toBeVisible();
      const meter = page.locator('summary').filter({ hasText: copy('Last request context 25%', '上次请求上下文用量 25%') });
      await meter.focus(); await meter.press('Enter');
      await expect(page.getByText(copy('32000 / 128000 input tokens · console-model', '输入词元 32000 / 128000 · console-model'), { exact: true })).toBeVisible();
      const message = page.getByRole('textbox', { name: locale === 'zh' ? '消息' : 'Message', exact: true });
      holdCompact = true;
      await message.fill('/compact'); await message.press('Enter');
      await expect(page.getByText(copy('Submitting compaction request…', '正在提交上下文压缩请求…'), { exact: true })).toBeVisible();
      await expect.poll(() => !!releaseCompact).toBe(true);
      releaseCompact!();
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
