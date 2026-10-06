import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test('workspace panel browses real files and runs a real PTY, including collapse and fullscreen', async ({ page }, info) => {
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  const toggle = page.locator('[data-preview-toggle]');
  await toggle.click();
  const panel = page.locator('[data-workbench]');
  await expect(panel).toBeVisible();
  await expect(panel.getByRole('button', { name: /New terminal/ })).toBeEnabled();
  await expect.poll(async () => (await panel.boundingBox())!.x + (await panel.boundingBox())!.width).toBeLessThanOrEqual(1440);
  await page.screenshot({path: info.outputPath('workbench-start.png')});
  await panel.getByRole('button', { name: /Workspace files/ }).click();
  await panel.getByRole('button', { name: /^src\/$/ }).click();
  await panel.getByRole('button', { name: 'hello.txt' }).click();
  await expect(panel.locator('pre')).toHaveText('Workspace preview works.');
  await panel.getByRole('button', { name: 'Start', exact: true }).click();
  await panel.getByRole('button', { name: /New terminal/ }).click();
  await expect(panel.locator('.xterm')).toBeVisible();
  await panel.locator('.xterm-helper-textarea').fill("printf 'RUSTX_%s\\n' TERMINAL");
  await panel.locator('.xterm-helper-textarea').press('Enter');
  // xterm's renderer paints to the browser DOM, independent of transport echoes.
  await expect.poll(async () => panel.locator('.xterm-rows').textContent()).toContain('RUSTX_TERMINAL');
  await page.getByRole('button', { name: 'Fullscreen', exact: true }).click();
  await expect(page.locator('[data-sidebar-right-panel]')).toHaveAttribute('data-sidebar-right-panel','fullscreen');
  await page.getByRole('button', { name: 'Restore', exact: true }).click();
  await toggle.click(); await expect(panel).toBeHidden(); await toggle.click(); await expect(panel.locator('.xterm')).toBeVisible();
  await expect.poll(async () => panel.locator('.xterm-rows').textContent()).toContain('RUSTX_TERMINAL');
  await expect.poll(async () => (await panel.boundingBox())!.x + (await panel.boundingBox())!.width).toBeLessThanOrEqual(1440);
  await page.screenshot({path: info.outputPath('workbench-terminal.png')});
  await panel.getByRole('button', { name: 'Close terminal 1' }).click();
  await expect(panel.getByRole('tab')).toHaveCount(0);
});

test('Chinese narrow workspace panel opens as a full-width page and closes', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'zh'));
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await expect(panel.getByRole('button', { name: /工作区文件/ })).toBeVisible();
  await expect(panel.getByRole('button', { name: /新建终端/ })).toBeEnabled();
  await expect(page.locator('[data-sidebar-right-panel]')).toHaveAttribute('data-sidebar-right-panel', 'fullscreen');
  await page.locator('aside[data-sidebar-right-open]').getByRole('button', { name: '切换工作区面板' }).click();
  await expect(panel).toBeHidden();
});
