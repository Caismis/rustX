import { expect, test, type Locator } from '@playwright/test';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
const gap = async (a: Locator, b: Locator) => {
  const first = (await a.boundingBox())!, next = (await b.boundingBox())!;
  return next.y - first.y - first.height;
};
for (const width of [1440, 390]) for (const theme of ['light', 'dark'] as const) test.describe(`${width}px ${theme}`, () => {
  test.use({ hasTouch: width === 390, isMobile: width === 390 });
  for (const kind of ['workflow', 'descriptor', 'descriptor-tools', 'status']) test(`${kind} keeps spacing at native seat boundaries`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.addInitScript(theme => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', 'en'); }, theme);
    await page.goto(`${origin}/test/fixtures/agent.html?mode=chat-spacing&case=${kind}`);
    await expect(page).toHaveTitle('Session A — rustX');
    if (kind === 'workflow') {
      const tool = page.locator('[data-tool-call-id="spacing-bash"]'), cards = page.locator('[data-workflow-run-id]');
      await expect(tool).toBeVisible(); await expect(cards).toHaveCount(2);
      await page.screenshot({ path: `/tmp/rustx-chat-spacing-${kind}-${width}-${theme}.png` });
      expect(await gap(tool, cards.first())).toBe(4);
      const toggle = tool.locator('[data-disclosure-row]'); await toggle.press('Enter');
      await expect(tool).toContainText('/workspace/A');
      expect(await gap(tool, cards.first())).toBe(4);
      await toggle.press('Enter'); expect(await gap(cards.first(), cards.last())).toBe(4);
    } else if (kind.startsWith('descriptor')) {
      const header = page.locator('[data-turn-process]'), answer = page.locator('[data-canonical-locator] [data-assistant-message]'), tail = page.locator('[data-turn-tail]');
      await expect(page.getByRole('heading', { name: 'Canonical answer' })).toBeVisible();
      await page.screenshot({ path: `/tmp/rustx-chat-spacing-${kind}-${width}-${theme}.png` });
      expect(await gap(page.getByRole('article', { name: 'Your message' }), header)).toBe(16);
      if (kind === 'descriptor-tools') {
        const tool = page.locator('[data-tool-call-id="spacing-deferred-bash"]');
        expect(await gap(header, tool)).toBe(16);
        expect(await gap(tool, answer)).toBe(12);
      } else expect(await gap(header, answer)).toBe(16);
      expect(await gap(answer, tail)).toBe(12);
    } else {
      const user = page.getByRole('article', { name: 'Your message' }), statuses = page.locator('[data-agent-status]');
      await expect(statuses).toHaveCount(2);
      expect(await gap(user, statuses.first())).toBe(4);
      expect(await gap(page.locator('[data-tool-call-id="flow-bash"]'), statuses.last())).toBe(4);
      const toggle = statuses.last().locator('[data-disclosure-row]'); await toggle.press('Enter');
      await expect(statuses.last().locator('[data-status-section]')).toBeVisible();
      await toggle.press('Enter');
      await page.evaluate(() => window.chatFlowFixture.settle());
      const process = page.locator('[data-turn-process]'); await process.press('Enter');
      expect(await gap(process, statuses.last())).toBeGreaterThanOrEqual(16);
      await page.screenshot({ path: `/tmp/rustx-chat-spacing-${kind}-${width}-${theme}.png` });
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
});
