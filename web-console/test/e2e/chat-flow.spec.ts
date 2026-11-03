import { expect, test } from '@playwright/test';
import { translator } from '../../src/locale/translation';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const width of [1440, 390]) for (const theme of ['light', 'dark'] as const) {
  const locale = width === 390 ? 'zh' : 'en';
  test.describe(`${width}px ${theme}`, () => {
  test.use({ hasTouch: width === 390, isMobile: width === 390 });
  test(`Harness chat flow, disclosure and settlement at ${width}px in ${theme}`, async ({ page }) => {
    const errors: string[] = [], t = translator(locale);
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: theme });
    await page.clock.setFixedTime(new Date('2026-11-03T01:01:05Z'));
    await page.addInitScript(({ theme, locale }) => {
      localStorage.setItem('rustx-appearance-v1', theme);
      localStorage.setItem('rustx-locale-v1', locale);
    }, { theme, locale });
    await page.goto(`${origin}/test/fixtures/agent.html?mode=chat-flow`);
    await expect(page).toHaveTitle('Session A — rustX');
    await expect(page.getByText('梳理对话流的布局，让思考、工具调用和最终回复更容易阅读。', { exact: true })).toBeVisible();
    const reasoning = page.locator('[data-variant="think"]');
    const read = page.locator('[data-tool-call-id="flow-read"]');
    const bash = page.locator('[data-tool-call-id="flow-bash"]');
    const running = page.locator('[data-chat-running]');
    await expect(read).toBeVisible(); await expect(bash).toBeVisible();
    await expect(running).toHaveCount(1);
    await expect(page.locator('[data-turn-process]')).toHaveCount(0);
    const gap = async (first: typeof read, second: typeof read) => {
      const a = (await first.boundingBox())!, b = (await second.boundingBox())!;
      return b.y - a.y - a.height;
    };
    expect(await gap(reasoning, read)).toBe(4);
    expect(await gap(read, bash)).toBe(4);
    expect(await read.locator('[data-disclosure-row]').evaluate(element => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
    const reservedHeight = (await running.boundingBox())!.height;
    expect(reservedHeight).toBe(32);
    for (const size of [10, 22, 14]) {
      await page.evaluate(size => window.chatFlowFixture.fontSize(size), size);
      await expect.poll(async () => (await running.boundingBox())!.height).toBe(32 + size - 14);
      expect(await gap(reasoning, read)).toBe(4);
      expect(await gap(read, bash)).toBe(4);
    }

    // The whole row is a keyboard target; Inspect is visible while it owns focus.
    const readToggle = read.locator('[data-disclosure-row]');
    await readToggle.focus(); await readToggle.press('Enter');
    await expect(readToggle).toHaveAttribute('aria-expanded', 'true');
    await expect(read.getByRole('button', { name: t('tools:tool-card.inspect'), exact: true })).toHaveCSS('opacity', '1');
    if (width === 390) {
      await readToggle.evaluate(element => (element as HTMLElement).blur());
      await expect(read.getByRole('button', { name: t('tools:tool-card.inspect'), exact: true })).toHaveCSS('opacity', '1');
    }
    await expect(read).toContainText('export function Conversation()');
    await readToggle.press('Enter'); await expect(readToggle).toHaveAttribute('aria-expanded', 'false');
    await reasoning.getByRole('button').focus(); await reasoning.getByRole('button').press('Space');
    await expect(reasoning).toHaveAttribute('data-expanded', 'true');
    await reasoning.getByRole('button').press('Space');
    await page.evaluate(() => window.chatFlowFixture.reply());
    const heading = page.getByRole('heading', { name: '对话流已整理', exact: true });
    const reply = page.locator('[data-chat-group-part="response"]').filter({ has: heading }).last();
    await expect(heading).toBeVisible();
    expect(await gap(bash, reply)).toBe(12);
    expect((await running.boundingBox())!.height).toBe(reservedHeight);
    await page.screenshot({ path: `/tmp/rustx-chat-flow-${width}-${theme}-live.png` });

    await page.evaluate(() => window.chatFlowFixture.settle());
    await expect(running).toHaveCount(0);
    const process = page.locator('[data-turn-process]');
    const duration = t('agent:duration.minutes-seconds', { m: 1, s: 5 });
    await expect(process).toHaveText(t('agent:copy.took-value', { p0: duration }));
    await expect(process).toHaveAttribute('aria-expanded', 'false');
    expect(await gap(process, reply)).toBe(12);
    await expect(page.getByRole('heading', { name: '对话流已整理', exact: true })).toBeVisible();
    await process.focus(); await process.press('Enter');
    await expect(process).toHaveAttribute('aria-expanded', 'true');
    const activity = page.locator('[data-process-activity]:visible');
    expect(await gap(process, activity)).toBe(16);
    await activity.focus(); await activity.press('Enter');
    await expect(read).toBeVisible(); await expect(bash).toBeVisible();
    expect(await gap(reasoning, read)).toBe(4);
    expect(await gap(read, bash)).toBe(4);
    await page.screenshot({ path: `/tmp/rustx-chat-flow-${width}-${theme}-settled.png` });
    await activity.press('Enter'); await process.focus(); await process.press('Enter');
    await expect(read).toBeHidden();
    await expect(page.getByRole('heading', { name: '对话流已整理', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(await page.evaluate(() => window.chatFlowFixture.outlineReads())).toBeLessThanOrEqual(4);
    expect(errors).toEqual([]);
  });
  });
}
