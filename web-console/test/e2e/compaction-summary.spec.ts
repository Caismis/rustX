import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
// Every locale, theme and width of the Context summary card from a stable
// native snapshot; compaction.spec.ts proves the native workflow itself.
const description = {
  en: 'This projection supplies model context. Canonical conversation facts retain their original execution ownership.',
  zh: '此投影提供模型上下文。规范会话事实保留其原始执行归属。',
} as const;
for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark']) for (const width of [1440, 390]) {
  test(`context summary card ${locale} ${theme} ${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(([theme, locale]) => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', locale); }, [theme, locale]);
    await page.goto(`${origin}/test/fixtures/agent.html?mode=compaction`);
    const card = page.locator('[data-compaction-marker]');
    const toggle = card.getByRole('button');
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await expect(toggle).toContainText('已完成的工作');
    await expect(toggle).toContainText('Continue with the original conversation context.');
    await toggle.scrollIntoViewIfNeeded();
    await card.screenshot({ path: info.outputPath('collapsed.png') });
    await toggle.focus(); await page.keyboard.press('Enter');
    await expect(card.getByRole('heading', { name: '已完成的工作' })).toBeVisible();
    await expect(card.locator('[data-compaction-body]').getByText('Continue with the original conversation context.')).toBeVisible();
    await expect(card.getByText(description[locale], { exact: true })).toBeVisible();
    expect(await card.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await card.screenshot({ path: info.outputPath('expanded.png') });
    await page.keyboard.press('Space');
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });
}
