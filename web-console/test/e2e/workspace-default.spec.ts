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
    const showNavigation = async () => { if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click(); };
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
