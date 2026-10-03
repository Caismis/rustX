import { test, expect } from '@playwright/test';
import { choose } from './shell-actions';
const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test('workspace primary/menu actions, keyboard, failure, retry and localization fit narrow headers', async ({ page }, testInfo) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${fixtureOrigin}/test/fixtures/agent.html?mode=desktop`);
  await expect(page).toHaveTitle('rustX Agent reference');
  await expect(page.getByLabel('Canonical conversation')).toBeVisible();
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Keep this draft');
  const primary = page.getByRole('button', { name: 'Open workspace', exact: true });
  await primary.focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('status').filter({ hasText: 'Launcher started' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).desktop.launches)).toEqual([{ scope: { authorityId: 'fixture-host', endpoint: 'ws://127.0.0.1:8080/' }, target: { session_id: 'A', active_node: 'node-A' }, application: 'files' }]);
  const menu = page.getByRole('button', { name: 'Choose workspace application' });
  await menu.focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitem', { name: 'File manager' })).toBeFocused();
  await page.keyboard.press('ArrowDown'); await expect(page.getByRole('menuitem', { name: 'Visual Studio Code' })).toBeFocused();
  await page.keyboard.press('Enter'); await expect(menu).toBeFocused();
  expect(await page.evaluate(() => (window as any).desktop.launches.map((row: any) => row.application))).toEqual(['files', 'code']);
  await menu.click();
  await page.getByRole('menuitem', { name: 'Check applications again' }).click();
  await expect(page.getByRole('menuitem', { name: 'File manager' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).desktop.catalogs)).toEqual([false, false, false, false, true]);
  expect(await page.evaluate(() => (window as any).desktop.launches.length)).toBe(2);
  await page.keyboard.press('Escape');
  await page.evaluate(() => { (window as any).desktop.failure = true; });
  await primary.click(); await expect(page.getByRole('alert').filter({ hasText: 'Could not open' })).toBeVisible();
  await page.getByText('Launch details', { exact: true }).click(); await expect(page.getByText('Error: Desktop adapter refused the launch', { exact: true })).toBeVisible();
  await page.evaluate(() => { (window as any).desktop.failure = false; (window as any).desktop.unavailable = true; });
  await page.getByRole('button', { name: 'Check applications again' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'cannot verify a local desktop' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).desktop.launches.length)).toBe(3);
  await page.evaluate(() => { (window as any).desktop.unavailable = false; });
  await page.getByRole('button', { name: 'Check applications again' }).click();
  expect(await page.evaluate(() => (window as any).desktop.catalogs)).toEqual([false, false, false, false, true, false, true, true]);
  await page.keyboard.press('Escape'); await expect(menu).toBeFocused();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
  await page.getByRole('button', { name: '关闭设置', exact: true }).click();
  await expect(page.getByRole('button', { name: '打开工作区', exact: true })).toBeVisible();
  for (const width of [1440, 390]) {
    if (width === 390) await page.getByRole('button', { name: '收起侧边栏', exact: true }).click();
    await page.setViewportSize({ width, height: 1000 });
    if (width === 390) {
      await expect(page.locator('[data-harness-frame]')).toHaveAttribute('data-sidebar-collapsed', 'true');
      await expect.poll(() => page.locator('[data-harness-frame]').evaluate(el => parseFloat(getComputedStyle(el).gridTemplateColumns))).toBeLessThan(100);
    }
    const control = page.getByRole('button', { name: '打开工作区', exact: true });
    await expect(control).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`desktop-${width}.png`) });
  }
  await expect(page.getByRole('textbox', { name: '消息', exact: true })).toHaveValue('Keep this draft');
  expect(await page.evaluate(() => (window as any).desktop.requests().filter((method: string) => /^(turn\/|tool\/)/.test(method)))).toEqual([]);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('real native Session metadata selects its canonical cwd without model work or activation by desktop resolution', async () => {
  const { startDogfood } = await import('./dogfood-server');
  const { AppServerHost } = await import('../../../tui/src/app-server/host');
  const { LocalWorkspaceHost } = await import('../../host/workspaces');
  const { DesktopAdapter } = await import('../../host/desktop');
  const { readDesktopSession } = await import('../../host/desktop-session');
  const { join } = await import('node:path');
  const { realpathSync, readFileSync } = await import('node:fs');
  const fixture = await startDogfood();
  const native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
  const launches: import('../../host/desktop').DesktopProcess[] = [];
  const adapter = new DesktopAdapter({ platform: 'linux', env: { PATH: '/usr/bin', DISPLAY: ':fixture' }, executable: path => path === '/usr/bin/xdg-open' ? path : undefined,
    launch: async spec => { launches.push(spec); return { status: 'spawned' }; } });
  let pause = false, reached!: () => void, release!: () => void;
  const entered = new Promise<void>(resolve => { reached = resolve; });
  const held = new Promise<void>(resolve => { release = resolve; });
  const host = new LocalWorkspaceHost({ ...JSON.parse(readFileSync(fixture.hostConfigFile, 'utf8')), nativeFilesystem: 'shared', metadataFile: join(fixture.directory, 'desktop-workspaces.json') }, adapter, async (...args) => { if (pause) { reached(); await held; } return readDesktopSession(...args); });
  try {
    const created = await native.createSession({ cwd: fixture.workspaceB });
    const target = { session_id: created.session.id, active_node: created.session.active_node };
    const scope = await host.listWorkspaces();
    await host.openWorkspace(scope, target, 'files');
    expect(launches.map(spec => spec.args)).toEqual([[realpathSync(fixture.workspaceB)]]);
    await expect(host.openWorkspace(scope, { ...target, active_node: 'retired-node' }, 'files')).rejects.toThrow('target changed');
    await expect(host.openWorkspace(scope, { ...target, session_id: 'missing-session' }, 'files')).rejects.toThrow();
    pause = true;
    const opening = host.openWorkspace(scope, target, 'files'); await entered;
    expect(launches).toHaveLength(1);
    const preview = await native.previewSessionDeletion(target.session_id);
    if (preview.status !== 'preview') throw new Error('Expected native deletion preview');
    expect((await native.deleteSession(target.session_id, preview.preview.target_revision)).status).toBe('deleted');
    release(); await expect(opening).rejects.toThrow();
    expect(launches).toHaveLength(1);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { host.close(); await native.shutdown(); await fixture.stop(false); }
});
