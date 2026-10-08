import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose, connectionAction } from './shell-actions';

for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`compaction continuation, questionnaire and Harness summary ${locale} ${theme}`, async ({ page }) => {
    const fixture = await startDogfood('web_compaction');
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const draft = 'Draft survives native maintenance ' + 'Keep these additional historical facts. '.repeat(512);
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
      // An idle Context seat renders nothing; no request occupancy is disclosed.
      await expect(page.locator('[data-context-seat]')).toHaveCount(0);
      await input.fill('/compact');
      await expect(page.getByRole('option', { name: copy('Compact compact', '压缩 compact'), exact: true })).toBeVisible();
      await input.press('Enter');
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
      // A measured request still discloses no occupancy in the Web.
      await expect(page.locator('meter')).toHaveCount(0);
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
      await message.fill(draft);
      await expect(message).toBeEditable();
      if (locale === 'en' && theme === 'light') {
        await connectionAction(page, 'Reconnect');
        await expect(message).toBeEditable();
        await expect(message).toHaveValue(draft);
        await expect(page.getByText('Compacting context…', { exact: true })).toBeVisible();
      }
      await fixture.release('manual-summary');
      await expect(page.getByText(locale === 'zh' ? '上下文已压缩' : 'Context compacted', { exact: true })).toBeVisible();
      await expect(message).toHaveValue(draft);
      const marker = page.locator('[data-compaction-marker]').getByRole('button');
      await expect(marker).toHaveCount(1);
      await expect(marker).toContainText('The user supplied compaction-evidence-435. Preserve that fact.');
      await expect(page.locator('[data-compaction-body]')).toHaveCount(0);
      await marker.focus(); await marker.press('Enter');
      await expect(page.locator('[data-compaction-body]').getByText('The user supplied compaction-evidence-435. Preserve that fact.', { exact: true })).toBeVisible();
      await expect(page.locator('[data-compaction-body]').getByText(copy('A condensed record of earlier context used to continue this conversation.', '先前上下文的精简记录，用于继续当前对话。'), { exact: true })).toBeVisible();
      await expect(marker).toHaveAttribute('aria-expanded', 'true');
      await marker.press('Space');
      await expect(marker).toHaveAttribute('aria-expanded', 'false');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await message.press('Enter');
      await expect(page.getByRole('heading', { name: 'Continue after compaction?' })).toBeVisible();
      await page.getByRole('radio', { name: 'Continue', exact: true }).click();
      await page.getByRole('button', { name: copy('Submit', '提交'), exact: true }).click();
      await fixture.gate('automatic-summary');
      await expect(page.getByText(copy('Compacting context…', '正在压缩上下文…'), { exact: true })).toBeVisible();
      await fixture.release('automatic-summary');
      await expect(page.getByText('Question answered after compaction.', { exact: true })).toBeVisible();
      await expect(page.locator('[data-context-seat]')).toHaveCount(0);
      const latest = page.locator('[data-chat-latest]');
      if (await latest.isVisible()) await latest.click();
      await expect(page.getByText('Question answered after compaction.', { exact: true })).toBeInViewport();
      await expect(marker).toHaveCount(2);
      await marker.last().click();
      await expect(page.locator('[data-compaction-body]').getByText("Preserve compaction-evidence-435 and the user's instruction to continue.", { exact: true })).toBeVisible();
      await page.screenshot({ path: `/tmp/rustx-compaction-${locale}-${theme}.png` });
      await expect(page.getByText(copy('Assistant recovery details', '助手恢复详情'), { exact: true })).toHaveCount(0);
      expect(errors).toEqual([]);
      await expect(page.getByRole('region', { name: copy('Questionnaire', '问卷'), exact: true })).toHaveCount(0);
      await message.fill('Stop a partial reply'); await message.press('Enter');
      await fixture.gate('stop-partial');
      await expect(page.getByRole('heading', { name: 'Partial answer' })).toBeVisible();
      await page.getByRole('button', { name: copy('Stop', '停止'), exact: true }).click();
      await expect(page.getByRole('button', { name: copy('Stopped', '已停止'), exact: true })).toBeVisible();
      // Release the abandoned provider stream only after native cancellation.
      // Late output must not replace the frozen partial or enter the next turn.
      await fixture.release('stop-partial');
      await expect(page.getByRole('heading', { name: 'Partial answer' })).toHaveCount(1);
      await expect(page.getByText('This text was already released.', { exact: true })).toBeVisible();
      await expect(page.getByText('This must not appear after stopping.', { exact: true })).toHaveCount(0);
      await expect(page.getByText(copy('Assistant recovery details', '助手恢复详情'), { exact: true })).toHaveCount(0);
      if (await latest.isVisible()) await latest.click();
      await page.screenshot({ path: `/tmp/rustx-stopped-${locale}-${theme}.png` });
      await message.fill('Continue after stopping'); await message.press('Enter');
      await expect(page.getByText('Conversation continued after stopping.', { exact: true })).toBeVisible();
      await expect(page.getByText('This must not appear after stopping.', { exact: true })).toHaveCount(0);
      expect(errors).toEqual([]);
      passed = true;
    } finally { await fixture.stop(passed); }
  });
}
