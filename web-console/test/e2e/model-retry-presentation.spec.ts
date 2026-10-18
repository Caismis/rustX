import { test, expect, type Locator, type Page } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

// The retry notice's presentation in every locale, theme and width, from
// stable native Trace states (test/fixtures/agent.tsx retry modes). The native
// timeout, retry, placement, Stop and late-output semantics are
// model-retry.spec.ts's two real workflows (en light 1440, zh dark 390).
const copy = {
  en: { active: 'Retrying model request (1)', settled: 'Model request retried (1)' },
  zh: { active: '正在重试模型请求（1）', settled: '已重试模型请求（1）' },
} as const;

/** Relative luminance of a computed CSS color, 0 (black) to 1 (white). */
async function luminance(element: Locator, property: 'color' | 'background-color') {
  return element.evaluate((node, property) => {
    let target: Element | null = node;
    let value = getComputedStyle(node).getPropertyValue(property);
    // A transparent background shows its nearest painted ancestor.
    while (property === 'background-color' && target && /rgba\(.*,\s*0\)$|transparent/.test(value)) {
      target = target.parentElement;
      value = target ? getComputedStyle(target).getPropertyValue(property) : 'rgb(255, 255, 255)';
    }
    const [r, g, b] = value.match(/[\d.]+/g)!.slice(0, 3).map(Number).map(channel => {
      const c = channel / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  }, property);
}

async function open(page: Page, mode: 'retry-active' | 'retry-settled', locale: 'en' | 'zh', theme: 'light' | 'dark', width: number) {
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(([theme, locale]) => {
    localStorage.setItem('rustx-appearance-v1', theme);
    localStorage.setItem('rustx-locale-v1', locale);
  }, [theme, locale]);
  await page.goto(`${origin}/test/fixtures/agent.html?mode=${mode}`);
  const notices = page.locator('[data-model-retry]');
  await expect(notices).toHaveCount(1);
  return notices.first();
}

async function presented(page: Page, notice: Locator, theme: 'light' | 'dark') {
  expect(await page.locator('body').evaluate(body => body.hasAttribute('data-ds-dark-theme'))).toBe(theme === 'dark');
  const summary = notice.locator('summary');
  // Readable in its theme: light text on dark, dark text on light.
  const text = await luminance(summary, 'color');
  const ground = await luminance(summary, 'background-color');
  expect(theme === 'dark' ? text > ground : text < ground).toBe(true);
  expect((Math.max(text, ground) + 0.05) / (Math.min(text, ground) + 0.05)).toBeGreaterThan(3);
  // Within the viewport, with no horizontal page overflow.
  const box = (await summary.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  // A disclosure, operable by pointer and keyboard.
  await expect(notice).not.toHaveAttribute('open');
  await summary.click();
  await expect(notice).toHaveAttribute('open');
  await summary.focus();
  await page.keyboard.press('Enter');
  await expect(notice).not.toHaveAttribute('open');
}

for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 390]) {
  test(`retry notice active and settled ${locale} ${theme} ${width}`, { tag: '@presentation' }, async ({ page }) => {
    const active = await open(page, 'retry-active', locale, theme, width);
    await expect(active.locator('summary')).toHaveText(copy[locale].active);
    await expect(active).toHaveAttribute('data-active', 'true');
    await presented(page, active, theme);

    const settled = await open(page, 'retry-settled', locale, theme, width);
    await expect(settled.locator('summary')).toHaveText(copy[locale].settled);
    await expect(settled).not.toHaveAttribute('data-active');
    // Settled retry history sits in the reply's seat, before the reply.
    const reply = page.getByText('Recovered after timeout.', { exact: true });
    await expect(reply).toBeVisible();
    expect(await settled.evaluate(node => {
      const seat = node.closest('[data-chat-anchor-key]');
      const answer = seat?.querySelector('article');
      return !!answer && !!(node.compareDocumentPosition(answer) & Node.DOCUMENT_POSITION_FOLLOWING);
    })).toBe(true);
    await presented(page, settled, theme);
  });
}
