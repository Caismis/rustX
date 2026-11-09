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

test('a floating file navigator replaces its own preview without moving the panel or stealing an open copy', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await expect(page).toHaveTitle('Session A — rustX');
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await panel.getByRole('button', { name: /^Workspace files/ }).click();
  await panel.getByRole('button', { name: 'docs', exact: true }).click();
  await panel.getByRole('button', { name: 'guide.md', exact: true }).click();
  const existing = panel.getByRole('tab', { name: /^guide.md/ });
  const existingTabId = await existing.getAttribute('data-dockkit-tab');
  const existingPaneId = await existing.evaluate(node => node.closest('[data-dockkit-pane]')!.getAttribute('data-dockkit-pane'));
  const retainedCopy = panel.locator(`[data-dockkit-host="dock"] [data-dockkit-content="${existingTabId}"]`);
  await panel.getByRole('tab', { name: /^Files/ }).click();
  await panel.getByRole('button', { name: 'sample.pdf', exact: true }).click();
  await expect(panel.locator('[data-pdf-page="1"] canvas')).toBeVisible();
  const chip = panel.getByRole('tab', { name: /^sample.pdf/ });
  await chip.hover(); const box = (await chip.boundingBox())!;
  await page.mouse.move(box.x + 25, box.y + 14); await page.mouse.down();
  await page.mouse.move(480, 200, { steps: 12 }); await page.mouse.up();
  const floating = panel.locator('[data-dockkit-float]');
  await expect(floating).toHaveCount(1);
  await expect(floating.locator('[data-pdf-page="1"] canvas')).toBeVisible();
  // The default float is narrow and its tree fills the preview. Widen it so
  // the navigator stays open, making retained tree focus observable too.
  const initialWidth = (await floating.boundingBox())!.width;
  const corner = (await floating.locator('[data-dockkit-float-resize]').boundingBox())!;
  await page.mouse.move(corner.x + corner.width / 2, corner.y + corner.height / 2); await page.mouse.down();
  await page.mouse.move(corner.x + corner.width / 2 + 320, corner.y + corner.height / 2 + 200, { steps: 8 }); await page.mouse.up();
  await expect.poll(async () => (await floating.boundingBox())!.width).toBe(initialWidth + 320);
  const identity = await floating.getAttribute('data-dockkit-float'), rectangle = await floating.boundingBox();
  const tabs = await panel.getByRole('tab').count();
  await floating.getByRole('button', { name: 'Show workspace files', exact: true }).click();
  await floating.getByRole('button', { name: 'docs', exact: true }).click();
  await floating.getByRole('button', { name: 'diagram.png', exact: true }).click();
  await expect(floating.locator('[data-image-preview] img')).toBeVisible();
  await expect(floating.locator('[data-pdf-preview]')).toHaveCount(0);
  await expect(floating).toHaveAttribute('data-dockkit-float', identity!);
  expect(await floating.boundingBox()).toEqual(rectangle);
  await expect(panel.getByRole('tab')).toHaveCount(tabs);
  await expect(panel.locator(`[data-dockkit-tab="${existingTabId}"]`)).toHaveCount(1);
  await expect(retainedCopy).toHaveAttribute('data-dockkit-pane', existingPaneId!);
  await expect(retainedCopy.getByRole('heading', { name: 'Rendered document', exact: true })).toBeVisible();
  await expect(floating.getByRole('button', { name: 'docs', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await expect(floating.getByRole('button', { name: 'diagram.png', exact: true })).toBeFocused();
  await floating.getByRole('button', { name: 'guide.md', exact: true }).click();
  await expect(floating.getByRole('heading', { name: 'Rendered document', exact: true })).toBeVisible();
  await expect(panel.locator(`[data-dockkit-tab="${existingTabId}"]`)).toHaveCount(1);
  await expect(retainedCopy).toHaveAttribute('data-dockkit-pane', existingPaneId!);
  await expect(retainedCopy.getByRole('heading', { name: 'Rendered document', exact: true })).toBeVisible();
  await expect(floating).not.toHaveAttribute('data-dockkit-content', existingTabId!);
  await expect(floating).toHaveAttribute('data-dockkit-float', identity!);
  expect(await floating.boundingBox()).toEqual(rectangle);
  await expect(panel.getByRole('tab')).toHaveCount(tabs);
  await expect(floating.getByRole('button', { name: 'guide.md', exact: true })).toBeFocused();
  await page.screenshot({ path: info.outputPath('floating-navigator-replacement.png') });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});
