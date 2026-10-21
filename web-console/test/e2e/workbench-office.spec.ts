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
    await page.route('**/product-host/workbench-fixture/workbench', async route => {
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
