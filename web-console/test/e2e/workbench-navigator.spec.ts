import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const width of [1440, 390]) test(`DSH workspace navigator replaces files and retains pane state (${width})`, async ({ page }, info) => {
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await expect(page).toHaveTitle('Session A — rustX');
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await panel.getByRole('button', { name: /^Workspace files/ }).click();
  await panel.getByRole('button', { name: 'docs', exact: true }).click();
  await panel.getByRole('button', { name: 'diagram.png', exact: true }).click();
  const image = panel.locator('[data-image-preview] img');
  await expect(image).toBeVisible();
  await expect(image).toHaveAttribute('data-pan', 'none');
  if (width === 1440) {
    const frame = panel.locator('[data-document-zoom-frame]');
    const box = (await frame.boundingBox())!; await page.mouse.move(box.x + box.width / 2, box.y + box.height - 20);
    await panel.getByRole('button', { name: 'Zoom', exact: true }).click();
    await page.getByRole('menuitem', { name: '150%', exact: true }).click();
    await expect(image).toHaveAttribute('data-pan', 'available');
    const port = panel.locator('[data-document-zoom-scrollport]');
    await port.evaluate(node => { node.scrollLeft = 100; });
    const before = await port.evaluate(node => node.scrollLeft), bounds = (await image.boundingBox())!;
    await page.mouse.move(Math.max(bounds.x, box.x) + 60, Math.max(bounds.y, box.y) + 50);
    await page.mouse.down(); await page.mouse.move(Math.max(bounds.x, box.x) + 20, Math.max(bounds.y, box.y) + 50, { steps: 4 }); await page.mouse.up();
    expect(await port.evaluate(node => node.scrollLeft)).toBeGreaterThan(before + 20);
    await panel.getByRole('button', { name: 'Fullscreen', exact: true }).click();
  }
  await panel.getByRole('button', { name: 'Show workspace files', exact: true }).click();
  const navigator = panel.locator('[data-files-navigator]');
  await expect(navigator).toBeVisible();
  await expect(navigator.getByRole('button', { name: 'docs', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await expect(navigator.getByRole('button', { name: 'diagram.png', exact: true })).toHaveAttribute('aria-current', 'page');
  let resized: string | null = null;
  if (width === 1440) {
    await expect(navigator).toHaveAttribute('data-files-navigator-mode', 'push');
    const edge = navigator.getByRole('separator'); await edge.focus(); await edge.press('ArrowLeft');
    resized = await edge.getAttribute('aria-valuenow'); expect(Number(resized)).toBe(280);
  }
  const tabs = await panel.getByRole('tab').count();
  await navigator.getByRole('button', { name: 'guide.md', exact: true }).click();
  await expect(panel.getByRole('heading', { name: 'Rendered document' })).toBeVisible();
  await expect(panel.getByRole('tab')).toHaveCount(tabs);
  await expect(panel.getByRole('tab', { name: /^diagram.png/ })).toHaveCount(0);
  if (width === 390) {
    await expect(navigator).toHaveCount(0);
    await panel.getByRole('button', { name: 'Show workspace files', exact: true }).click();
  } else {
    await expect(navigator.getByRole('separator')).toHaveAttribute('aria-valuenow', resized!);
    await expect(navigator.getByRole('button', { name: 'guide.md', exact: true })).toBeFocused();
  }
  await expect(navigator.getByRole('button', { name: 'docs', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await page.screenshot({ path: info.outputPath(`navigator-${width}.png`) });
  await navigator.getByRole('button', { name: 'docs', exact: true }).focus(); await page.keyboard.press('Escape');
  await expect(navigator).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Show workspace files', exact: true })).toBeFocused();
  await panel.getByRole('button', { name: 'View as', exact: true }).click(); await page.getByRole('menuitem', { name: 'Text', exact: true }).click();
  await expect(panel.locator('[data-textpreview-line="1"]')).toContainText('---');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});
