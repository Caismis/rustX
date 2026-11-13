import { test, expect } from '@playwright/test';
import { captureEvidence } from './screenshot';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const width of [1440, 390]) test(`dual-sidebar primary routes preserve the Home editor and native owners at ${width}px`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${origin}/test/fixtures/shell.html?navigation`);
  await expect(page).toHaveTitle('Session A — rustX');
  const rail = page.getByRole('navigation', { name: 'Primary navigation' });
  const home = rail.getByRole('button', { name: 'Home', exact: true });
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(home).toHaveAttribute('aria-current', 'page');
  await expect(rail).toBeVisible();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  await editor.fill('Keep the same conversation, selection and local attachment');
  await page.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'retained.txt', mimeType: 'text/plain', buffer: Buffer.from('retained') });
  await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(1);
  const incarnation = await page.locator('[data-editor-incarnation]').getAttribute('data-editor-incarnation');
  const facts = await page.evaluate(() => window.sessionFixture.navigationFacts());
  const requests = await page.evaluate(() => window.sessionFixture.requests());
  if (width === 390) await home.click();
  await expect(page.getByRole('tree', { name: 'Session browser' })).toBeVisible();
  if (width === 390) expect((await page.locator('#home-sidebar').boundingBox())!.x).toBe(56);
  await captureEvidence(page, `home-expanded-${width}`);
  await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await expect(home).toHaveAttribute('aria-expanded', 'false');
  await expect(home).toBeFocused();
  await expect(page.getByRole('tree', { name: 'Session browser' })).toHaveCount(0);
  await expect(rail).toBeVisible();
  await captureEvidence(page, `home-collapsed-${width}`);
  await home.click();
  await expect(home).toHaveAttribute('aria-expanded', 'true');
  if (width === 390) {
    await expect(page.locator('main')).toHaveAttribute('inert', '');
    await expect(home).toBeInViewport();
    await page.keyboard.press('Escape');
    await expect(home).toBeFocused();
  }
  for (const destination of ['Workflows', 'Scheduled Tasks', 'Settings']) {
    await rail.getByRole('button', { name: destination, exact: true }).click();
    await expect(rail.locator('[aria-current=page]')).toHaveCount(1);
    await expect(rail.getByRole('button', { name: destination, exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('tree', { name: 'Session browser' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'New Conversation', exact: true })).toHaveCount(0);
    await expect(page.locator('[data-rightbar-col]')).toBeHidden();
    await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
    if (destination === 'Settings') {
      const settings = page.getByRole('region', { name: 'Settings', exact: true });
      await expect(settings).toBeVisible();
      await expect(settings.getByRole('button', { name: 'Reload configuration', exact: true })).toBeEnabled();
      await expect(settings.getByRole('alert')).toHaveCount(0);
      await page.keyboard.press('Escape');
      await expect(settings).toBeVisible();
    }
    await captureEvidence(page, `${destination.toLowerCase().replaceAll(' ', '-')}-${width}`);
    await home.click();
    await expect(editor).toHaveValue('Keep the same conversation, selection and local attachment');
    await expect(page.locator('[data-editor-incarnation]')).toHaveAttribute('data-editor-incarnation', incarnation!);
    await expect(page.getByText('retained.txt', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Queue', exact: true })).toBeEnabled();
    expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(facts);
  }
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(delta.filter(row => /^(turn\/|thread\/(start|resume|fork)$|inbound\/(queue|edit|remove)$|session\/(detach|submissionIntent|switchNode|uploadPrepare)$)/.test(row.method))).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
  expect(errors).toEqual([]);
});

for (const width of [1440, 390]) test(`fullscreen workbench leaves primary navigation accessible and retains its file at ${width}px`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${origin}/test/fixtures/workbench.html?draft`);
  await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await editor.fill('Keep my workspace and file reading position');
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await panel.getByRole('button', { name: /Workspace files/ }).click();
  await panel.getByRole('button', { name: 'src', exact: true }).click();
  await panel.getByRole('button', { name: 'hello.txt', exact: true }).click();
  await expect(panel.locator('pre')).toHaveText('Workspace preview works.');
  const fullscreen = panel.getByRole('button', { name: 'Fullscreen', exact: true });
  if (await fullscreen.isVisible()) await fullscreen.click();
  const rail = page.getByRole('navigation', { name: 'Primary navigation' });
  const home = rail.getByRole('button', { name: 'Home', exact: true });
  expect(await home.evaluate(el => { const rect = el.getBoundingClientRect(); return el.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2)); })).toBe(true);
  await captureEvidence(page, `workbench-fullscreen-with-rail-${width}`);
  await rail.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(panel).toBeHidden();
  const main = (await page.locator('main').boundingBox())!;
  expect(main.x).toBe(56); expect(main.width).toBe(width - 56);
  await home.click();
  await expect(panel.locator('pre')).toHaveText('Workspace preview works.');
  await expect(editor).toHaveValue('Keep my workspace and file reading position');
  await expect(panel.getByRole('tab', { selected: true })).toContainText('hello.txt');
  await expect(rail).toBeVisible();
  expect(errors).toEqual([]);
});

test('the Home sidebar restores its resized width across collapse, primary pages and reload', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${origin}/test/fixtures/shell.html`);
  const sidebar = page.locator('#home-sidebar');
  const handle = (await page.locator('[data-side=sidebar]').boundingBox())!;
  await page.mouse.move(handle.x + handle.width / 2, 400); await page.mouse.down();
  await page.mouse.move(handle.x + handle.width / 2 + 60, 400, { steps: 6 }); await page.mouse.up();
  await expect.poll(async () => Math.round((await sidebar.boundingBox())!.width)).toBe(340);
  await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  await page.getByRole('button', { name: 'Home', exact: true }).click();
  expect(Math.round((await sidebar.boundingBox())!.width)).toBe(340);
  await page.getByRole('button', { name: 'Workflows', exact: true }).click();
  await page.getByRole('button', { name: 'Home', exact: true }).click();
  await expect(sidebar).toBeVisible();
  expect(Math.round((await sidebar.boundingBox())!.width)).toBe(340);
  await page.reload(); await expect(sidebar).toBeVisible();
  expect(Math.round((await sidebar.boundingBox())!.width)).toBe(340);
});

for (const platform of ['win32', 'darwin'] as const) for (const width of [1440, 390]) test(`platform chrome leaves the rail and fullscreen workbench usable on ${platform} at ${width}px`, async ({ page }) => {
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${origin}/test/fixtures/workbench.html?draft`);
  await page.evaluate(platform => {
    const root = document.documentElement;
    root.dataset.platform = platform;
    if (platform === 'win32') { root.dataset.windowsTitlebar = ''; root.style.setProperty('--dsh-windows-titlebar-height', '40px'); }
  }, platform);
  const rail = page.getByRole('navigation', { name: 'Primary navigation' });
  const home = rail.getByRole('button', { name: 'Home', exact: true });
  await home.click();
  const sidebar = (await page.locator('#home-sidebar').boundingBox())!;
  expect(sidebar.x).toBe(56); expect(sidebar.y).toBe(platform === 'win32' ? 40 : 0);
  expect((await home.boundingBox())!.y).toBeGreaterThanOrEqual(platform === 'win32' ? 40 : 58);
  if (width === 390) await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
  await page.locator('[data-preview-toggle]').click();
  const panel = page.locator('[data-workbench]');
  await panel.getByRole('button', { name: /Workspace files/ }).click();
  await panel.getByRole('button', { name: 'src', exact: true }).click();
  await panel.getByRole('button', { name: 'hello.txt', exact: true }).click();
  await expect(panel.locator('pre')).toHaveText('Workspace preview works.');
  const fullscreen = panel.getByRole('button', { name: 'Fullscreen', exact: true });
  if (await fullscreen.isVisible()) await fullscreen.click();
  const bounds = (await panel.boundingBox())!;
  expect(bounds.x).toBe(56); expect(bounds.y).toBe(platform === 'win32' ? 40 : 0);
  expect(bounds.width).toBe(width - 56);
  expect(await home.evaluate(el => { const r = el.getBoundingClientRect(); return el.contains(document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2)); })).toBe(true);
  if (platform === 'darwin') {
    const first = panel.locator('[data-dockkit-host="dock"][data-dockkit-column="0"]:visible');
    await expect(first).toHaveCSS('--dsh-dockkit-strip-inline-start', '32px');
    await page.evaluate(() => { document.documentElement.dataset.fullscreen = ''; });
    await expect(first).toHaveCSS('--dsh-dockkit-strip-inline-start', '10px');
  }
  await captureEvidence(page, `platform-${platform}-${width}`);
  await rail.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(panel).toBeHidden();
  expect((await page.locator('main').boundingBox())!.y).toBe(platform === 'win32' ? 40 : 0);
  await home.click();
  await expect(panel.locator('pre')).toHaveText('Workspace preview works.');
});
