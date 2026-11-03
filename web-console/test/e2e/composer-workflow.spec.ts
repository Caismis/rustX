import { expect, test } from '@playwright/test';
import { translator } from '../../src/locale/translation';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const width of [1440, 390]) for (const theme of ['light', 'dark'] as const) test.describe(`${width}px ${theme}`, () => {
  test.use({ hasTouch: width === 390, isMobile: width === 390, timezoneId: 'Asia/Shanghai' });
  test('composer focus, native send acknowledgement and Workflow header layout', async ({ page }) => {
    const locale = width === 390 ? 'zh' : 'en', t = translator(locale), errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.clock.setFixedTime(new Date('2026-11-03T06:43:00Z'));
    await page.addInitScript(({ theme, locale }) => {
      localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', locale);
    }, { theme, locale });
    await page.goto(`${origin}/test/fixtures/agent.html?mode=composer&workflow-runs`);
    await expect(page).toHaveTitle('Session A — rustX');
    await expect(page.getByText('运行两次工作流并汇总结果。', { exact: true })).toBeVisible();
    const panels = page.locator('[data-workflow-run-id]'), completed = panels.nth(0), cancelled = panels.nth(1);
    await expect(panels).toHaveCount(2);
    const first = completed.locator('[data-disclosure-row]').first(), second = cancelled.locator('[data-disclosure-row]').first();
    await expect(first).toHaveAttribute('aria-expanded', 'false');
    await expect(second).toHaveAttribute('aria-expanded', 'true');
    const countText = t('common:workflow.nodes', { count: 3 });
    for (const header of [first, second]) {
      const title = header.getByText(header === first ? 'test_flow' : 'test_flow_cancelled_with_a_long_name_for_layout', { exact: true });
      const count = header.getByText(countText, { exact: true });
      const labelBox = (await title.boundingBox())!, countBox = (await count.boundingBox())!, headerBox = (await header.boundingBox())!;
      expect(countBox.x - labelBox.x - labelBox.width).toBeGreaterThanOrEqual(6);
      expect(countBox.x + countBox.width).toBeLessThan(headerBox.x + headerBox.width);
      expect(await header.evaluate(element => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
    }
    await expect(second).toContainText(t('common:state.cancelled'));
    await expect(cancelled.locator('[data-workflow-node]')).toHaveCount(3);
    await expect(cancelled.locator('[data-workflow-node="format_answer"] svg')).toHaveCount(1);
    await second.focus(); await second.press('Enter');
    await expect(cancelled.locator('[data-workflow-node]')).toHaveCount(0);
    await page.screenshot({ path: `/tmp/rustx-composer-workflow-${width}-${theme}-collapsed.png` });
    await first.focus(); await first.press('Space');
    await expect(completed.locator('[data-workflow-node]')).toHaveCount(3);
    await expect(completed).toContainText(t('common:workflow.kind.template'));
    await expect(completed).toContainText(t('common:workflow.kind.list'));
    await page.screenshot({ path: `/tmp/rustx-composer-workflow-${width}-${theme}-expanded.png` });

    const input = page.getByRole('textbox', { name: t('agent:agent-composer.message'), exact: true });
    const card = page.locator('[data-composer-card]'), primary = page.locator('[data-composer-primary]');
    if (width === 390) expect(parseFloat(await input.evaluate(element => getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16);
    await input.fill('保持草稿和光标位置');
    await input.evaluate(element => { const input = element as HTMLTextAreaElement; input.setSelectionRange(3, 3); input.blur(); });
    // The 8px top pad belongs to the card, outside the textarea's hit target.
    await card.click({ position: { x: 30, y: 4 } });
    await expect(input).toBeFocused();
    expect(await input.evaluate(element => (element as HTMLTextAreaElement).selectionStart)).toBe(3);
    await input.fill('第一行'); await input.press('Shift+Enter'); await input.pressSequentially('第二行');
    await expect(input).toHaveValue('第一行\n第二行');
    expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual([]);
    await page.getByRole('button', { name: t('commands:menu.add'), exact: true }).click();
    await input.press('Escape');
    await expect(input).toBeFocused();
    await page.evaluate(() => window.composerFixture.holdSubmission());
    await primary.click();
    await expect(primary).toHaveAccessibleName(t('agent:submission.sending'));
    await expect(primary).toHaveAttribute('aria-busy', 'true');
    await expect(primary).toBeDisabled(); await expect(primary).toHaveCSS('opacity', '1');
    await input.press('Enter');
    expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual(['turn/start']);
    await page.screenshot({ path: `/tmp/rustx-composer-workflow-${width}-${theme}-sending.png` });
    await page.evaluate(() => window.composerFixture.acknowledgeSubmission());
    await expect(input).toHaveValue('');
    await expect(primary).not.toHaveAttribute('aria-busy', 'true');
    await page.evaluate(() => window.composerFixture.running(true));
    await expect(primary).toHaveAccessibleName(t('agent:submission.stop'));
    await expect(primary).toBeEnabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
});
