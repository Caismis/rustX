import { test, expect } from '@playwright/test';
for (const width of [1440, 390]) {
  test(`managed output is plain recorded information at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 1000 });
    await page.goto('http://127.0.0.1:5174/test/fixtures/managed-output.html');
    await expect(page).toHaveTitle('Managed output inspection');
    const inspector = page.getByLabel('Trace record inspector');
    await inspector.getByRole('tab', { name: 'Result' }).click();
    await expect(inspector.getByText('Partial', { exact: true })).toBeVisible();
    const locator = '/private/rustx-managed-output/example/tasks/result.output';
    const value = inspector.getByText(locator, { exact: true });
    await expect(value).toBeVisible();
    expect(await value.evaluate(el => el.tagName)).toBe('DD');
    await expect(value.locator('a, button')).toHaveCount(0);
    await expect(inspector.getByText(`cannot append ${locator}: recorded I/O failure`, { exact: true })).toBeVisible();
    expect(await value.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
    await page.screenshot({ path: `/tmp/rustx-368-managed-output-${width}.png` });
  });
}
