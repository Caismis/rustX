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
    const copy = <T extends string | RegExp>(en: T, zh: T) => locale === 'zh' ? zh : en;
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
      const dock = page.locator('[data-composer-dock]');
      const contextMeter = dock.getByRole('button', { name: copy(/% of context used$/, /^上下文已用 /), exact: true });
      await expect(contextMeter).toBeVisible();
      const pressureBefore = Number.parseInt(await contextMeter.innerText(), 10);
      const usageBefore = await dock.locator('button').allTextContents();
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
      await expect(page.locator('[data-compaction-marker]')).toBeVisible();
      await expect(dock.getByRole('button', { name: /(% of context used$|^上下文已用 )/ })).toBeVisible();
      const compactedOccupancy = dock.getByRole('button', { name: /(% of context used$|^上下文已用 )/ });
      expect(Number.parseInt(await compactedOccupancy.innerText(), 10)).toBeLessThan(pressureBefore);
      await compactedOccupancy.click();
      await expect(page.getByRole('dialog').locator('[class*="figures"]')).toHaveText(/^~/);
      await page.keyboard.press('Escape');
      // Compaction estimates are separate from reported cumulative billing.
      const usageAfter = await dock.locator('button').allTextContents();
      expect(usageAfter.slice(0, 2)).toEqual(usageBefore.slice(0, 2));
      await expect(message).toHaveValue(draft);
      const marker = page.locator('[data-compaction-marker]').getByRole('button');
      await expect(marker).toHaveCount(1);
      await expect(marker).toHaveText(copy(/compact.*Compacted \d+ history items \(~\d+ tokens\)/, /compact.*已压缩 \d+ 条历史记录（约 \d+ 词元）/));
      await marker.click();
      await expect(page.getByText('The user supplied compaction-evidence-435. Preserve that fact.', { exact: true })).toBeVisible();
      await expect(marker).toHaveAttribute('aria-expanded', 'true');
      await marker.click();
      await expect(marker).toHaveAttribute('aria-expanded', 'false');
      await marker.scrollIntoViewIfNeeded();
      await page.mouse.move(0, 0);
      const checkpointBox = (await marker.boundingBox())!
      const dockBox = (await dock.boundingBox())!;
      const viewport = page.viewportSize()!;
      const top = Math.max(0, checkpointBox.y - 72);
      await page.screenshot({ path: `/tmp/rustx-manual-compaction-${locale}-${theme}.png`, clip: {
        x: Math.max(0, checkpointBox.x - 24), y: top,
        width: Math.min(viewport.width - Math.max(0, checkpointBox.x - 24), checkpointBox.width + 48),
        height: Math.min(viewport.height, dockBox.y + dockBox.height + 8) - top,
      } });
      if (locale === 'en' && theme === 'light') {
        // Exercise a fresh browser attachment, not only an in-place reconnect.
        // Native maintenance has committed; reloading cannot replay /compact.
        await page.reload();
        // This fixture uses an explicit Remote credential held only in memory,
        // rather than the dev launcher's authenticated local bootstrap.
        await connectRemote(page, fixture.endpoint, fixture.token);
        await expect(message).toBeEditable();
        await expect(marker).toHaveCount(1);
        await expect(dock.getByRole('button', { name: /(% of context used$|^上下文已用 )/ })).toBeVisible();
        await marker.click();
        await expect(page.getByText('The user supplied compaction-evidence-435. Preserve that fact.', { exact: true })).toBeVisible();
        await marker.click();
        // Composer drafts are browser-memory state; restore the fixture's
        // continuation input after creating a fresh page.
        await message.fill(draft);
      }
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
      await expect(page.getByText("Preserve compaction-evidence-435 and the user's instruction to continue.", { exact: true })).toBeVisible();
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
