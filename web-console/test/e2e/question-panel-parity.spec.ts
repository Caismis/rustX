import { expect, test } from '@playwright/test';
import { translator } from '../../src/locale/translation';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const width of [1440, 390]) for (const theme of ['light', 'dark'] as const) test.describe(`${width}px ${theme}`, () => {
  test.use({ hasTouch: width === 390, isMobile: width === 390 });
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => {
      const send = WebSocket.prototype.send;
      WebSocket.prototype.send = function(data) {
        if (typeof data === 'string' && JSON.parse(data).method === 'interaction/respond') document.documentElement.dataset.interactionResponses = String(Number(document.documentElement.dataset.interactionResponses ?? 0) + 1);
        return send.call(this, data);
      };
    });
  });
  test('ask_user has an independent read-only option panel and compact transcript', async ({ page }) => {
    const locale = width === 390 ? 'zh' : 'en', tx = translator(locale), errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.addInitScript(({ theme, locale }) => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', locale); }, { theme, locale });
    await page.goto(`${origin}/test/fixtures/agent.html?mode=question-history`);
    await expect(page).toHaveTitle('Session A — rustX');
    const live = page.getByRole('region', { name: tx('interactions:questionnaire.questionnaire'), exact: true });
    await live.getByRole('textbox').fill('Retain the pending draft');
    await live.getByRole('button', { name: tx('interactions:questionnaire.minimize'), exact: true }).click();
    await page.locator('[data-turn-process]').press('Enter');
    await page.locator('[data-process-activity]:visible').first().press('Enter');
    const row = page.locator('[data-tool-call-id="ask-history"]'), arrow = row.getByRole('button', { name: tx('tools:ask.row-title'), exact: true });
    await expect(row).toContainText(tx('tools:ask.answered', { answered: 2, total: 3 }));
    await row.getByRole('button', { name: tx('tools:ask.review'), exact: true }).press('Enter');
    await expect(arrow).toHaveAttribute('aria-expanded', 'false');
    const panel = page.locator('[data-question-record]');
    await expect(panel.getByRole('radio', { name: 'Simplify', exact: true })).toBeChecked();
    await expect(panel.getByRole('radio', { name: 'Keep native', exact: true })).toBeDisabled();
    await expect(panel.locator('strong')).toHaveText('UI only');
    await expect(panel.getByRole('textbox')).toHaveCount(0);
    await panel.getByRole('button', { name: tx('interactions:questionnaire.next-question'), exact: true }).click();
    await expect(panel.getByRole('checkbox', { name: 'Browser', exact: true })).toBeChecked();
    await expect(panel.getByRole('checkbox', { name: 'Unit', exact: true })).toBeChecked();
    await expect(panel.getByRole('checkbox', { name: 'Unit', exact: true })).toBeDisabled();
    await panel.getByRole('button', { name: tx('interactions:questionnaire.next-question'), exact: true }).click();
    await expect(panel).toContainText(tx('tools:ask.skipped'));
    await expect(panel.getByRole('button', { name: tx('interactions:questionnaire.submit'), exact: true })).toHaveCount(0);
    await panel.getByRole('button', { name: tx('interactions:questionnaire.previous-question'), exact: true }).click();
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.screenshot({ path: `/tmp/rustx-ask-record-panel-${width}-${theme}.png` });
    await page.locator('[data-tool-call-id="call-ask"]').getByRole('button', { name: tx('tools:ask.reopen'), exact: true }).press('Enter');
    await expect(panel).toHaveCount(0);
    await expect(live.getByRole('textbox')).toHaveValue('Retain the pending draft');
    await arrow.press('Enter');
    await expect(row.locator('dl')).toBeVisible();
    await expect(row.getByText('Simplify', { exact: true })).toHaveCount(1);
    expect(await page.evaluate(() => Number(document.documentElement.dataset.interactionResponses ?? 0))).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
  for (const mode of ['questionnaire', 'approval']) test(`${mode} native details follow Harness without losing the pending draft`, async ({ page }) => {
    const locale = width === 390 ? 'zh' : 'en', tx = translator(locale), errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.addInitScript(({ theme, locale }) => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', locale); }, { theme, locale });
    await page.goto(`${origin}/test/fixtures/agent.html?mode=${mode}`);
    await expect(page).toHaveTitle('Session A — rustX');
    if (mode === 'questionnaire') {
      const panel = page.getByRole('region', { name: tx('interactions:questionnaire.questionnaire'), exact: true });
      await panel.getByRole('radio', { name: 'Keep native', exact: true }).click();
      await panel.getByRole('textbox').fill('Keep this draft');
      await panel.getByRole('button', { name: tx('interactions:questionnaire.minimize'), exact: true }).click();
      await page.locator('[data-tool-call-id="call-ask"]').getByRole('button', { name: tx('tools:ask.reopen'), exact: true }).press('Enter');
      await expect(panel.getByRole('textbox')).toHaveValue('Keep this draft');
      await expect(panel).toContainText('2 / 2');
      await expect(panel.getByRole('button', { name: tx('interactions:questionnaire.submit'), exact: true })).toBeEnabled();
    } else {
      const panel = page.getByRole('region', { name: tx('interactions:approval-takeover.approval'), exact: true });
      await expect(panel.getByText('bash', { exact: true })).toBeVisible();
      await expect(panel.getByText('Developer approval required', { exact: true })).toBeVisible();
      await expect(panel.locator('pre')).toHaveText('printf rustX');
      expect(await panel.locator('pre').evaluate(element => ({ font: getComputedStyle(element).fontSize, margin: getComputedStyle(element).margin }))).toEqual({ font: '13px', margin: '0px' });
      await expect(panel.getByRole('button', { name: tx('interactions:approval-takeover.allow-once'), exact: true })).toBeEnabled();
      await expect(panel.getByRole('button', { name: tx('interactions:approval-takeover.deny'), exact: true })).toBeEnabled();
      await expect(page.getByRole('button', { name: tx('interactions:interactions.cancel-interaction'), exact: true })).toHaveCount(0);
      await panel.locator('..').screenshot({ path: `/tmp/rustx-approval-actions-${width}-${theme}.png` });
    }
    expect(await page.evaluate(() => Number(document.documentElement.dataset.interactionResponses ?? 0))).toBe(0);
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.screenshot({ path: `/tmp/rustx-${mode}-parity-${width}-${theme}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
});
