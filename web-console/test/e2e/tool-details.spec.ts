import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const theme of ['light', 'dark'] as const) test(`Harness tool details: independent scroll, full output and copy ${theme}`, async ({ page, context }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto(`${origin}/test/fixtures/tool-details.html`);
  await expect(page).toHaveTitle('rustX Tool details');
  await page.evaluate(value => document.documentElement.dataset.theme = value, theme);
  const call = page.locator('[data-tool-call-id="long-bash"]');
  const toggle = call.getByRole('button', { name: /^bash/ });
  await toggle.focus();
  await page.keyboard.press('Enter');
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const terminal = call.locator('[data-terminal]');
  const banner = terminal.locator(':scope > div').nth(0);
  const output = terminal.locator(':scope > div').nth(1);
  await expect(output.getByText('result 24    preserved alignment', { exact: true })).toHaveCount(1);
  await expect(call.getByRole('button', { name: /more lines|Collapse output/ })).toHaveCount(0);
  const metrics = await terminal.evaluate(node => {
    const banner = node.children[0] as HTMLElement, output = node.children[1] as HTMLElement;
    const style = getComputedStyle(node), line = getComputedStyle(output.children[0]);
    return { font: getComputedStyle(output).fontSize, lineHeight: line.lineHeight, minHeight: line.minHeight, margin: style.marginTop, indent: style.marginLeft, radius: style.borderRadius,
      bannerHeight: banner.clientHeight, outputHeight: output.clientHeight, bannerScroll: banner.scrollHeight > banner.clientHeight, outputScroll: output.scrollHeight > output.clientHeight };
  });
  expect(metrics).toMatchObject({ font: '11px', lineHeight: '16px', minHeight: '18px', margin: '4px', indent: '4px', radius: '16px', bannerScroll: true, outputScroll: true });
  expect(metrics.bannerHeight).toBeLessThanOrEqual(150);
  expect(metrics.outputHeight).toBeLessThanOrEqual(224);
  await banner.evaluate(node => node.scrollTop = node.scrollHeight);
  await expect(terminal.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
  const bannerTop = await banner.evaluate(node => node.getBoundingClientRect().top);
  await output.evaluate(node => node.scrollTop = node.scrollHeight);
  expect(await banner.evaluate(node => node.getBoundingClientRect().top)).toBe(bannerTop);
  await terminal.getByRole('button', { name: 'Copy', exact: true }).click();
  await expect(terminal.getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toBe(Array.from({ length: 48 }, (_, index) => `result ${index + 1}    preserved alignment`).join('\n') + '\n');
  await page.mouse.move(0, 0);
  await call.screenshot({ path: `/tmp/rustx-tool-details-${theme}.png` });
  await page.getByRole('button', { name: 'Toggle running' }).click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(terminal).toHaveAttribute('data-running', '');
  await expect(terminal.getByRole('button', { name: /Copy/ })).toHaveCount(0);
  await page.getByRole('button', { name: 'Toggle running' }).click();
  await expect(terminal.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
  await toggle.focus(); await page.keyboard.press('Space');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await toggle.click();
  const generic = page.locator('[data-tool-call-id="generic"]');
  await generic.getByRole('button', { name: /^exa_search/ }).click();
  const sections = generic.locator('[class*="ioSection"]');
  expect(await sections.count()).toBe(2);
  for (const section of await sections.all()) expect(await section.evaluate(node => node.clientHeight <= 150 && node.scrollHeight > node.clientHeight)).toBe(true);
  await expect(generic.locator('[class*="ioDivider"]')).toHaveCount(1);
  for (const id of ['read', 'edit', 'search']) {
    const row = page.locator(`[data-tool-call-id="${id}"]`);
    await row.getByRole('button', { expanded: false }).click();
  }
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await call.screenshot({ path: `/tmp/rustx-tool-details-${theme}-390.png` });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});
