import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
// Every locale, theme and width of the Context summary card from a stable
// native snapshot; compaction.spec.ts proves the native workflow itself.
for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark']) for (const width of [1440, 390]) {
  test(`context summary card ${locale} ${theme} ${width}`, { tag: '@presentation' }, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(([theme, locale]) => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', locale); }, [theme, locale]);
    await page.goto(`${origin}/test/fixtures/agent.html?mode=compaction`);
    const card = page.locator('[data-compaction-marker]');
    const toggle = card.getByRole('button');
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await expect(toggle).toContainText(locale === 'zh' ? '上下文已压缩' : 'Context compacted');
    await expect(toggle).toContainText(locale === 'zh' ? '点击查看压缩摘要' : 'View compaction summary');
    await expect(card).not.toContainText('已完成的工作');
    await expect(page.locator('[data-projection-history]')).toHaveCount(0);
    await expect(page.locator('[data-composer-dock] [data-compaction-marker]')).toHaveCount(0);
    await expect(page.locator('[aria-label="' + (locale === 'zh' ? '规范对话' : 'Canonical conversation') + '"] [data-compaction-marker]')).toHaveCount(1);
    await toggle.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `/tmp/rustx-compaction-collapsed-${locale}-${theme}-${width}.png` });
    await toggle.focus(); await page.keyboard.press('Enter');
    await expect(card.getByRole('heading', { name: '已完成的工作' })).toBeVisible();
    await expect(card.locator('[data-compaction-body]').getByText('Continue with the original conversation context.')).toBeVisible();
    await expect(card).not.toContainText(locale === 'zh' ? '规范历史边界' : 'canonical frontier');
    expect(await card.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await page.screenshot({ path: `/tmp/rustx-compaction-expanded-${locale}-${theme}-${width}.png` });
    await page.keyboard.press('Space');
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    // The same native checkpoint keeps its seat as later conversation output
    // arrives, including after another compaction. It is never a sticky footer.
    const first = page.locator('[data-compaction-checkpoint="checkpoint-1"]');
    await page.evaluate(() => window.compactionFixture.reply());
    await expect(page.getByText('New reply 2', { exact: true })).toBeVisible();
    expect(await first.evaluate(el => !!(el.compareDocumentPosition(document.querySelector('[data-chat-anchor-key="message:prompt-2"]')!) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
    await page.evaluate(() => window.compactionFixture.compact());
    await expect(page.locator('[data-compaction-marker]')).toHaveCount(2);
    await page.evaluate(() => window.compactionFixture.reply());
    await expect(page.getByText('New reply 3', { exact: true })).toBeVisible();
    const second = page.locator('[data-compaction-checkpoint="checkpoint-2"]');
    expect(await second.evaluate(el => !!(el.compareDocumentPosition(document.querySelector('[data-chat-anchor-key="message:prompt-3"]')!) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
    await second.getByRole('button').click();
    await expect(second.getByText('Second preserved summary.')).toBeVisible();
    await second.getByRole('button').click();
    await page.screenshot({ path: `/tmp/rustx-compaction-position-${locale}-${theme}-${width}.png` });
  });
}
