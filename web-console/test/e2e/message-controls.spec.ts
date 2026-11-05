import { expect, test } from '@playwright/test';

declare global {
  interface Window { controlsAudit: { events: { previews: string[]; downloads: string[] }; reads(): number } }
}
const origin = 'http://127.0.0.1:' + (process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
for (const width of [1280, 390]) for (const theme of ['light', 'dark']) {
  test(`message controls follow DSH without extra action/caption rows (${width}px, ${theme})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.goto(origin + '/test/fixtures/message-refresh.html?controls');
    await expect(page).toHaveTitle('Message controls audit');
    const root = page.locator('[data-controls-audit]');
    await expect(root).toBeVisible();
    await expect(root).not.toContainText(/Steps used|Agent calls|Model calls|Job ·|Finite background Tool|Cancel interaction|View agent/);
    const tool = page.locator('[data-tool-call-id="bash"]');
    await tool.getByRole('button', { expanded: false }).click();
    await expect(tool.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
    await expect(tool).not.toContainText('job-controls');
    await page.locator('[data-inbound-source="agent"]').getByRole('button', { expanded: false }).click();
    await expect(page.locator('[data-inbound-source="agent"] p')).toHaveText('Recorded child result');
    await expect(root.getByRole('button', { name: 'View agent', exact: true })).toHaveCount(0);
    const file = root.getByRole('button', { name: 'Preview report.txt', exact: true });
    await expect(file).toBeVisible();
    await expect(root.getByText('Preview report.txt', { exact: true })).toHaveCount(0);
    await file.click();
    expect(await page.evaluate(() => window.controlsAudit.events.previews)).toEqual(['report.txt']);
    expect(await page.evaluate(() => window.controlsAudit.reads())).toBe(0);
    await root.getByRole('button', { name: 'Download report.txt', exact: true }).click();
    expect(await page.evaluate(() => window.controlsAudit.events.downloads)).toEqual(['report.txt']);
    expect(await page.evaluate(() => window.controlsAudit.events.previews)).toEqual(['report.txt']);
    await expect(root.getByText('Load attachment', { exact: true })).toHaveCount(0);
    await root.getByRole('button', { name: 'Load attachment preview.png', exact: true }).click();
    await expect(root.getByRole('img', { name: 'preview.png', exact: true })).toBeVisible();
    await root.getByRole('button', { name: 'Open image preview.png', exact: true }).click();
    expect(await page.evaluate(() => window.controlsAudit.events.previews)).toEqual(['report.txt', 'preview.png']);
    expect(await page.evaluate(() => window.controlsAudit.reads())).toBe(1);
    await expect(root.getByRole('status')).toHaveText('Loading…');
    await expect(root.getByRole('button', { name: 'Cancel interaction', exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
    await page.screenshot({ path: `/tmp/rustx-message-controls-${width}-${theme}.png`, fullPage: true });
  });
}
