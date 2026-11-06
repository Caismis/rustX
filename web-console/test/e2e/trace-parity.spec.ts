import { test, expect, type Locator } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { captureStable } from '../screenshot-stability';
import { compareScreenshot, decodePng } from '../screenshot-comparator';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const width of [1440, 390]) for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark']) {
  test(`native detail preserves Harness presentation at ${width} ${locale} ${theme}`, async ({ page }) => {
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 844 });
    await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
    const expected = new Map<string, Buffer>();
    for (const native of [false, true]) {
      await page.goto(`${origin}/test/fixtures/trajectory.html?parity${native ? '&native' : ''}`);
      if (theme === 'dark') await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
      await expect(page).toHaveTitle('Trajectory presentation contracts');
      const panel = page.getByRole('complementary', { name: locale === 'zh' ? '事件详情' : 'Event details' });
      const ledger = page.getByRole('table');
      const close = async () => panel.getByRole('button', { name: locale === 'zh' ? '关闭详情' : 'Close details' }).click();
      const tab = (en: string, zh: string) => panel.getByRole('tab', { name: locale === 'zh' ? zh : en, exact: true });
      const capture = async (name: string, target: Locator = panel) => {
        await page.evaluate(() => document.fonts.ready);
        const settled = await captureStable(async delay => { if (delay) await page.waitForTimeout(delay); return target.screenshot({ animations: 'disabled', caret: 'hide', scale: 'css' }); }, name);
        expect(settled.stable, settled.stable ? undefined : settled.report).toBe(true); if (!settled.stable) return;
        if (!native) expected.set(name, settled.png);
        else {
          const result = compareScreenshot({ referenceName: name, expected: decodePng(expected.get(name)!), actual: decodePng(settled.png), noisePolicy: [] });
          expect(result.ok, result.report).toBe(true);
          if (width === 1440 && locale === 'zh' && theme === 'dark') writeFileSync(`/tmp/rustx-dsh-${name}.png`, settled.png);
        }
      };
      await ledger.locator('[data-display-type="RecordRow"][data-owner="trace:4"]').click();
      const thinking = panel.getByRole('button', { name: locale === 'zh' ? '思考' : 'Thinking', exact: true });
      await expect(thinking).toBeVisible(); await expect(panel.locator('strong').filter({ hasText: /^working tree$/ })).toBeVisible();
      await expect(panel.getByRole('combobox')).toHaveCount(0); await expect(panel).not.toContainText('assistant-4');
      await capture('assistant-summary');
      await thinking.click(); await expect(thinking).toHaveAttribute('aria-expanded', 'false'); await capture('assistant-collapsed');
      await tab('Raw', '原始内容').click(); await expect(panel).toContainText('git diff --stat'); await capture('assistant-raw');
      await close(); await ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]').click();
      await expect(tab('Code', '代码')).toBeVisible(); await expect(panel.getByRole('heading',{level:3,name:'bash',exact:true})).toBeVisible(); await capture('tool-summary');
      await tab('Code', '代码').click(); await expect(panel.locator('pre')).toContainText('git diff --stat'); await capture('tool-source');
      await tab('Schema', 'Schema').click(); await expect(panel.getByRole('heading',{level:3,name:'bash',exact:true})).toBeVisible(); await expect(panel.getByRole('tree')).toBeVisible(); await capture('tool-schema');
      await close(); await ledger.locator('[data-request-owner="trace:3"]').click();
      await tab('Options', '选项').click(); await expect(panel.getByRole('tree')).toBeVisible(); await expect(panel).toContainText('historical-model'); await capture('request-options');
      await close(); await ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]').click();
      await tab('System Prompt', '系统提示词').click(); await expect(panel).toContainText('You are the historical agent.'); await capture('system-prompt');
      await tab('Tools', '工具').click(); await expect(panel.locator('details')).toHaveCount(1); await panel.locator('details summary').click(); await expect(panel.getByRole('tree')).toBeVisible(); await capture('tool-catalog');
      await expect(panel.getByRole('combobox')).toHaveCount(0); await expect(panel.getByRole('button', { name: /Read content|Read original value|Read next chunk/ })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    }
    expect(errors).toEqual([]);
  });
}
