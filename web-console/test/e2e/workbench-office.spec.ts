import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const locale of ['en', 'zh'] as const) {
  test(`Office sidebar renders hyperlinks and printer settings, with localized retry (${locale})`, async ({ page }, info) => {
    page.setDefaultTimeout(15_000);
    const errors: string[] = [], external: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => { if (request.url().includes('rustx-preview.invalid')) external.push(request.url()); });
    await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
    let rejectNext = false;
    await page.route('**/product-host/workbench-fixture/workbench-preview', async route => {
      if (rejectNext && route.request().postDataJSON().call?.request.kind === 'office') {
        rejectNext = false;
        await route.fulfill({ status: 400, json: { message: 'Error: archive_rejected' } });
      } else await route.continue();
    });
    await page.goto(`${origin}/test/fixtures/workbench.html`);
    await page.locator('[data-preview-toggle]').click();
    const panel = page.locator('[data-workbench]');
    await panel.getByRole('button', { name: locale === 'zh' ? /^工作区文件/ : /^Workspace files/ }).click();
    await panel.getByRole('button', { name: 'docs', exact: true }).click();
    await panel.getByRole('button', { name: 'linked.docx', exact: true }).click();
    await expect(panel.locator('.textLayer')).toContainText('rustX document preview');
    await expect(panel.locator('canvas')).toBeVisible();
    const refresh = panel.getByRole('button', { name: locale === 'zh' ? '刷新' : 'Refresh', exact: true });
    rejectNext = true;
    await refresh.click();
    await expect(panel.getByRole('alert')).toContainText(locale === 'zh' ? 'OOXML 压缩包被拒绝' : 'OOXML archive rejected');
    await expect(panel.getByRole('alert')).not.toContainText('WorkspaceHostError');
    await refresh.click();
    await expect(panel.locator('.textLayer')).toContainText('rustX document preview');
    await expect(panel.getByRole('alert')).toHaveCount(0);
    await panel.getByRole('tab', { name: locale === 'zh' ? /^文件/ : /^Files/ }).click();
    await panel.getByRole('button', { name: 'linked.pptx', exact: true }).click();
    await expect(panel.locator('.textLayer')).toContainText('rustX presentation preview');
    await expect(panel.locator('canvas')).toBeVisible();
    await page.screenshot({ path: info.outputPath(`office-sidebar-${locale}.png`) });
    expect(external).toEqual([]);
    expect(errors).toEqual([]);
  });
}

for (const width of [1440, 390]) test(`Office first page does not wait for later PDF pages (${width})`, async ({ page }, info) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: 1000 });
  await page.addInitScript(() => {
    const NativeWorker = window.Worker;
    const pending: (() => void)[] = [];
    let released = false;
    window.Worker = class extends NativeWorker {
      postMessage(message: any, options?: Transferable[] | StructuredSerializeOptions) {
        const send = () => { if (Array.isArray(options)) super.postMessage(message, options); else super.postMessage(message, options); };
        if (!released && message?.action === 'GetPage' && message.data?.pageIndex > 0) {
          pending.push(send); return;
        }
        send();
      }
    };
    Object.assign(window, { pdfPageGate: { count: () => pending.length, release() { released = true; pending.splice(0).forEach(send => send()); } } });
  });
  await page.goto(`${origin}/test/fixtures/workbench.html`);
  await expect(page).toHaveTitle('Session A — rustX');
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await panel.getByRole('button', { name: /^Workspace files/ }).click();
  await panel.getByRole('button', { name: 'docs', exact: true }).click();
  await panel.getByRole('button', { name: 'slides.pptx', exact: true }).click();
  await expect(panel.locator('[data-pdf-page="1"] .textLayer')).toContainText('slide 1');
  await expect(panel.locator('[data-pdf-page="1"] canvas')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).pdfPageGate.count())).toBeGreaterThan(0);
  await page.screenshot({ path: info.outputPath(`office-first-page-${width}.png`) });
  await page.evaluate(() => (window as any).pdfPageGate.release());
  await panel.locator('[data-pdf-page="2"]').scrollIntoViewIfNeeded();
  await expect(panel.locator('[data-pdf-page="2"] .textLayer')).toContainText('slide 2');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});
