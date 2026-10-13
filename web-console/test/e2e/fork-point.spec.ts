import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const theme of ['light', 'dark']) for (const width of [1440, 390]) {
  test(`fork boundary ${theme} ${width}`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(theme => { localStorage.setItem('rustx-appearance-v1', theme); localStorage.setItem('rustx-locale-v1', 'zh'); }, theme);
    await page.goto(`${origin}/test/fixtures/agent.html?mode=fork`);
    const point = page.getByRole('group', { name: '分叉点' });
    await expect(point).toBeVisible();
    await expect(point).toContainText('从「原始会话：探索历史分叉与上下文压缩的交互设计」分叉');
    await expect(page.getByText('分叉前的历史', { exact: true })).toHaveCount(0);
    await point.getByRole('button', { name: '查看来源' }).focus();
    await expect(point.getByRole('button')).toBeFocused();
    expect(await point.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    const after = page.getByText('Continue from this fork boundary.', { exact: true });
    expect((await point.boundingBox())!.y).toBeLessThan((await after.boundingBox())!.y);
    await point.screenshot({ path: info.outputPath('fork-point.png') });
    expect(errors).toEqual([]);
  });
}
