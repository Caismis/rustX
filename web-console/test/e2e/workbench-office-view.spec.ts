import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const { locale, theme, width } of [
  { locale: 'en', theme: 'dark', width: 1440 }, { locale: 'zh', theme: 'light', width: 1440 },
  { locale: 'zh', theme: 'dark', width: 390 }, { locale: 'en', theme: 'light', width: 390 },
]) test(`Office uses Harness continuous pages and floating zoom (${locale}-${theme}-${width})`, async ({ page }, info) => {
  page.setDefaultTimeout(15000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme as 'dark' | 'light', reducedMotion: 'reduce' });
  await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await expect(page).toHaveTitle('Workspace panel');
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  const openOffice = async (name: string) => {
    const converted = page.waitForResponse(response => response.url().endsWith('/workbench-preview')
      && response.request().method() === 'POST' && response.request().postDataJSON()?.call?.request?.kind === 'office'
      && response.request().postDataJSON()?.call?.request?.path === `docs/${name}`);
    await panel.getByRole('button', { name, exact: true }).click();
    const response = await converted;
    expect(response.ok()).toBe(true);
  };
  await panel.getByRole('button', { name: locale === 'zh' ? /^工作区文件/ : /^Workspace files/ }).click();
  await panel.getByRole('button', { name: 'docs', exact: true }).click();
  await openOffice('slides.pptx');
  const port = panel.locator('[data-document-zoom-scrollport]'), frame = panel.locator('[data-document-zoom-frame]');
  const first = panel.locator('[data-pdf-page="1"]'), second = panel.locator('[data-pdf-page="2"]');
  await expect(first.locator('.textLayer')).toContainText('slide 1');
  await expect(second.locator('.textLayer')).toContainText('slide 2');
  await expect(panel.locator('[data-pdf-page]')).toHaveCount(3);
  await expect(panel.getByRole('button', { name: locale === 'zh' ? '下一页' : 'Next', exact: true })).toHaveCount(0);
  await expect(panel.getByRole('button', { name: locale === 'zh' ? '查看方式' : 'View as', exact: true })).toHaveCount(0);
  expect(await port.evaluate(node => node.scrollWidth <= node.clientWidth + 1)).toBe(true);
  const a = (await first.boundingBox())!, b = (await second.boundingBox())!;
  expect(Math.abs(b.y - (a.y + a.height) - 12)).toBeLessThan(1);
  const reveal = async () => { const box = (await frame.boundingBox())!; await page.mouse.move(box.x + box.width / 2, box.y + box.height - 20); };
  await reveal();
  const controlsBox = (await panel.locator('[data-document-zoom-controls]').boundingBox())!;
  expect(Math.abs(controlsBox.width - 132)).toBeLessThan(1);
  const zoom = panel.getByRole('button', { name: locale === 'zh' ? '缩放' : 'Zoom', exact: true });
  await zoom.click(); await page.getByRole('menuitem', { name: '150%', exact: true }).click();
  await expect(frame).toHaveAttribute('data-document-zoom-mode', 'fixed');
  await expect(zoom).toContainText('150%');
  await reveal(); await zoom.click();
  await page.getByRole('menuitem', { name: locale === 'zh' ? '适合宽度' : 'Fit width', exact: true }).click();
  await expect(frame).toHaveAttribute('data-document-zoom-mode', 'fit-width');
  await second.scrollIntoViewIfNeeded();
  await expect(second.locator('.textLayer')).toContainText('slide 2');
  const saved = await port.evaluate(node => node.scrollTop);
  await panel.getByRole('tab', { name: locale === 'zh' ? /^文件/ : /^Files/ }).click();
  await openOffice('linked.docx');
  await expect(panel.locator('[data-pdf-page="1"] .textLayer')).toContainText('rustX document preview');
  await panel.getByRole('tab', { name: /^slides.pptx/ }).click();
  await expect(second.locator('.textLayer')).toContainText('slide 2');
  await expect.poll(() => port.evaluate((node, saved) => Math.abs(node.scrollTop - saved), saved)).toBeLessThan(2);
  await port.evaluate(node => { node.scrollTop = 0; });
  await reveal();
  await page.screenshot({ path: info.outputPath(`office-continuous-${locale}-${theme}-${width}.png`) });
  expect(errors).toEqual([]);
  expect(await page.locator('vite-error-overlay').count()).toBe(0);
});
