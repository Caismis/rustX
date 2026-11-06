import { test, expect, type Locator } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose } from './shell-actions';

/** Both elements sit in the same anchored transcript seat. */
async function expectSharedAnchor(reply: Locator, notice: Locator) {
  const anchor = (element: Locator) => element.evaluate(el => el.closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key') ?? null);
  await expect.poll(async () => {
    const seat = await anchor(reply);
    return seat !== null && seat === await anchor(notice);
  }).toBe(true);
}

// One native retry workflow per locale, each locale on the other theme and
// width: English light desktop (plus reload) and Chinese dark narrow. Both
// locales' retry copy across every theme and width is pinned by
// test/model-retry.test.tsx; the retry semantics do not vary by presentation.
for (const [locale, theme] of [['en', 'light'], ['zh', 'dark']] as const) {
  test(`native timeout retry disclosure ${locale} ${theme}`, { tag: locale === 'en' ? ['@smoke'] : [] }, async ({ page }) => {
    const fixture = await startDogfood('web_timeout_retry');
    let passed = false;
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    const copy = (en: string, zh: string) => locale === 'en' ? en : zh;
    try {
      await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
      await page.setViewportSize({ width: theme === 'dark' ? 390 : 1440, height: 1000 });
      await routeWorkspaceHost(page, fixture); await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token); const sessionId = await openEmptySession(page, fixture, 'Workspace A');
      if (locale === 'zh') {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置' }).click();
      }
      const input = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
      await input.fill('Timeout and recover'); await input.press('Enter'); await fixture.gate('retry-started');
      const notices = page.locator('[data-model-retry]');
      await expect(notices).toHaveCount(1);
      await expect(notices.first().locator('summary')).toContainText(copy('Retrying model request (1)', '正在重试模型请求（1）'));
      await expect(notices.first()).toHaveAttribute('data-active', 'true');
      await expect(notices.first()).not.toHaveAttribute('open');
      await notices.first().locator('summary').click();
      await expect(notices.first()).toContainText('Fixture model response timeout');
      await expect(page.getByRole('button', { name: copy('Stop', '停止'), exact: true })).toBeEnabled();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `/tmp/rustx-timeout-retry-${locale}-${theme}.png` });
      await fixture.release('retry-started');
      await expect(page.getByText('Recovered after timeout.', { exact: true })).toBeVisible();
      await expect(notices).toHaveCount(1);
      await expect(notices.first().locator('summary')).toContainText(copy('Model request retried (1)', '已重试模型请求（1）'));
      await expect(page.locator('[data-model-retry][data-active]')).toHaveCount(0);
      // Disclosure state belongs to the retry chain when its native publication
      // moves from the pending seat into the final message stream.
      await expect(notices.first()).toHaveAttribute('open');
      // The reply is first visible in its streaming seat; its durable message
      // seat, which the retry notice joins, follows. Wait for that placement.
      const recovered = page.getByText('Recovered after timeout.', { exact: true });
      await expectSharedAnchor(recovered, notices.first());
      expect(await notices.first().evaluate(el => {
        const reply = el.closest('[data-chat-anchor-key]')?.querySelector('article');
        return !!reply && !!(el.compareDocumentPosition(reply) & Node.DOCUMENT_POSITION_FOLLOWING);
      })).toBe(true);
      await notices.first().locator('summary').click();
      await expect(notices.first()).not.toHaveAttribute('open');
      await page.screenshot({ path: `/tmp/rustx-retry-position-${locale}-${theme}.png` });
      await input.fill('Stop during retry'); await input.press('Enter'); await fixture.gate('stop-retry');
      await expect(page.locator('[data-model-retry][data-active]')).toHaveCount(1);
      await page.getByRole('button', { name: copy('Stop', '停止'), exact: true }).click();
      await expect(page.getByRole('button', { name: copy('Send', '发送'), exact: true })).toBeVisible();
      await expect(page.locator('[data-model-retry][data-active]')).toHaveCount(0);
      await fixture.release('stop-retry');
      await expect(page.getByText('Late retry output must stay hidden.', { exact: true })).toHaveCount(0);
      await expect(input).toBeEditable();
      if (locale === 'en' && theme === 'light') {
        await page.reload(); await connectRemote(page, fixture.endpoint, fixture.token);
        const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
        if (await expand.isVisible()) await expand.click();
        await page.locator(`button[data-session-id="${sessionId}"]`).click();
        await expect(page.getByText('Recovered after timeout.', { exact: true })).toBeVisible();
        await expect(notices).toHaveCount(2);
        await expect(page.locator('[data-model-retry][data-active], [data-model-retry][open]')).toHaveCount(0);
        await expectSharedAnchor(page.getByText('Recovered after timeout.', { exact: true }), notices.first());
      }
      expect(errors).toEqual([]); passed = true;
    } finally { await fixture.stop(passed); }
  });
}
