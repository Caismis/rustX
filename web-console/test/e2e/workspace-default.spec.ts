import { test, expect } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote } from './shell-actions';
import { wireProbe } from './wire-probe';
function gate() { let resolve!: () => void; const promise = new Promise<void>(done => { resolve = done; }); return { promise, resolve }; }
for (const width of [1440, 390]) test(`startup and Home use registered Workspaces without losing the draft (${width})`, async ({ page }, info) => {
  const fixture = await startDogfood(); let passed = false;
  const native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
  const classification = gate(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ reducedMotion: 'reduce' });
  try {
    const a = (await native.createSession({ cwd: fixture.workspaceA })).session.id; await native.renameSession(a, 'Existing A');
    const b = (await native.createSession({ cwd: fixture.workspaceB })).session.id; await native.renameSession(b, 'Existing B');
    await routeWorkspaceHost(page, fixture);
    await page.route('**/product-host/classify', async route => { await classification.promise; await route.fallback(); });
    const wire = await wireProbe(page);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await input.fill('Keep typing while Workspaces load');
    await input.evaluate(element => { element.setAttribute('data-default-workspace-editor', ''); (element as HTMLTextAreaElement).setSelectionRange(2, 8); });
    const card = page.locator('[data-composer-card]'), before = await card.boundingBox();
    classification.resolve();
    const picker = page.getByRole('button', { name: 'Choose Workspace', exact: true });
    await expect(picker).toContainText('Workspace B');
    await expect(page.locator('[data-default-workspace-editor]')).toBeFocused();
    await expect(input).toHaveValue('Keep typing while Workspaces load');
    expect(await input.evaluate(element => [(element as HTMLTextAreaElement).selectionStart, (element as HTMLTextAreaElement).selectionEnd])).toEqual([2, 8]);
    expect(await card.boundingBox()).toEqual(before);
    expect(wire.requests.filter(request => request.method === 'session/create')).toHaveLength(0);
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
    await page.screenshot({ path: info.outputPath(`workspace-default-${width}.png`) });
    await picker.click(); await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
    await expect(picker).toContainText('Workspace A');
    const showNavigation = async () => { if (width === 390) await page.getByRole('button', { name: 'Home', exact: true }).click(); };
    await showNavigation(); await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
    await expect(picker).toContainText('Workspace A'); await expect(input).toHaveValue('Keep typing while Workspaces load');
    await showNavigation(); await page.locator(`button[data-session-id="${a}"]`).click();
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await showNavigation();
    await expect(page.getByRole('button', { name: 'Select Workspace Workspace A', exact: true })).toHaveAttribute('aria-current', 'page');
    await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
    await expect(picker).toContainText('Workspace A');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
    passed = true;
  } finally {
    classification.resolve(); await page.close(); await native.shutdown();
    const report = await fixture.stop(false); // Navigation deliberately makes no provider request.
    if (passed) expect(report.requestCount).toBe(0);
  }
});

for (const width of [1440, 390]) test(`unsent manual Workspace choice survives refresh and a fresh browser page (${width})`, async ({ page }, info) => {
  const fixture = await startDogfood();
  const native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
  let fresh: typeof page | undefined, passed = false;
  const errors: string[] = [];
  const observe = (surface: typeof page) => {
    surface.on('pageerror', error => errors.push(error.message));
    surface.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  };
  const showNavigation = async (surface: typeof page) => { if (width === 390) await surface.getByRole('button', { name: 'Home', exact: true }).click(); };
  const expectDraftA = async (surface: typeof page) => {
    await expect(surface.locator('#session-view')).toHaveAttribute('data-shell-route', 'new-conversation');
    await expect(surface.getByRole('button', { name: 'Choose Workspace', exact: true })).toContainText('Workspace A');
  };
  try {
    const a = (await native.createSession({ cwd: fixture.workspaceA })).session.id; await native.renameSession(a, 'Existing A');
    const b = (await native.createSession({ cwd: fixture.workspaceB })).session.id; await native.renameSession(b, 'Existing B');
    await page.setViewportSize({ width, height: 1000 }); observe(page);
    await routeWorkspaceHost(page, fixture); const wire = await wireProbe(page);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByRole('button', { name: 'Choose Workspace', exact: true })).toContainText('Workspace B');
    await showNavigation(page); await page.locator(`button[data-session-id="${b}"]`).click();
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await showNavigation(page);
    await page.getByRole('button', { name: 'Select Workspace Workspace A', exact: true }).click();
    await expectDraftA(page);
    expect(wire.requests.filter(request => request.method === 'session/create')).toHaveLength(0);
    await page.reload(); await connectRemote(page, fixture.endpoint, fixture.token); await expectDraftA(page);
    // Exercise the composer's chooser too, then restore the older Workspace A
    // even though Native still reports the more recent Session in Workspace B.
    const picker = page.getByRole('button', { name: 'Choose Workspace', exact: true });
    await picker.click(); await page.getByRole('menuitem', { name: 'Workspace B', exact: true }).click();
    await picker.click(); await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
    await expectDraftA(page);
    await expect.poll(async () => {
      const catalog = await (await fetch(`${fixture.workspaceHostUrl}/product-host/list`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).json();
      return catalog.workspaces.find((row: { id: string }) => row.id === catalog.preferredWorkspace)?.displayName;
    }).toBe('Workspace A');
    const context = page.context(); await page.close();
    fresh = await context.newPage(); await fresh.setViewportSize({ width, height: 1000 }); observe(fresh);
    await routeWorkspaceHost(fresh, fixture); const restartedWire = await wireProbe(fresh);
    // A new origin has no localStorage from the first page. The persisted Host
    // preference must restore A even when the Web carrier's port changes.
    await fresh.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? '5174'}/`); await connectRemote(fresh, fixture.endpoint, fixture.token); await expectDraftA(fresh);
    await expect(fresh).toHaveTitle(/rustX/i);
    await fresh.getByRole('textbox', { name: 'Message', exact: true }).fill('Restored Workspace A before sending');
    await expect(fresh.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
    expect(restartedWire.requests.filter(request => request.method === 'session/create')).toHaveLength(0);
    await expect(fresh.locator('[data-phase="connecting"], [data-phase="disconnected"]')).toHaveCount(0);
    await expect(fresh.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
    await fresh.screenshot({ path: info.outputPath(`remembered-workspace-${width}.png`) });
    passed = true;
  } finally {
    await fresh?.close(); if (!page.isClosed()) await page.close(); await native.shutdown();
    const report = await fixture.stop(false); if (passed) expect(report.requestCount).toBe(0);
  }
});
