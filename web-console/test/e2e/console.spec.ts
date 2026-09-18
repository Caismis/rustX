import { closeSessionView } from './shell-actions';
import { showInspector } from './shell-actions';
import { chooseWorkspace, connectionAction, closeSettings } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host.ts';
import { startDogfood } from './dogfood-server';
import { wireProbe } from './wire-probe';

test('two real rustX Sessions, browser loss, native interactions, raw wire, and cold configuration resolution', async ({ page }) => {
  const fixture = await startDogfood();
  const wire = await wireProbe(page);
  const closeView = async (id: string) => {
    const count = wire.responses.filter(row => row.method === 'session/detach').length;
    await closeSessionView(page, id);
    await expect.poll(() => wire.responses.filter(row => row.method === 'session/detach').length).toBe(count + 1);
  };
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const connect = async () => {
    await page.getByLabel('WebSocket endpoint').fill(`${fixture.endpoint}/`);
    await page.getByLabel('Transport token').fill(fixture.token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Connection', exact: true })).toHaveCount(0); await showInspector(page);
  };
  const send = async (text: string) => { await page.getByRole('textbox', { name: 'Message', exact: true }).fill(text); await page.getByRole('button', { name: 'Send', exact: true }).click(); };
  const reload = async () => { await page.reload(); await connect(); };
  let remote: AppServerHost | undefined;
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await expect(page).toHaveTitle('rustX Developer Console');
    await expect(page.locator('[data-harness-frame]')).toBeVisible();
    await connect();
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByLabel('Session location', { exact: true })).toHaveText(`${fixture.workspaceA}`);
    const idA = JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).SessionId as string;
    await chooseWorkspace(page, 'Workspace B'); await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByLabel('Session location', { exact: true })).toHaveText(`${fixture.workspaceB}`);
    const idB = JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).SessionId as string;
    await expect(page.getByRole('tree', { name: 'Session browser' })).toHaveCount(1);
    await expect(page.getByRole('tablist', { name: 'Open Session views' })).toHaveCount(0);
    await page.locator(`button[data-session-id="${idA}"]`).click();
    await send('Long action in A'); await fixture.gate('finish-a');
    await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    // Configuration source commits cannot rewrite a running admitted attempt.
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await settings.getByRole('tab', { name: 'Workspace', exact: true }).click();
    await settings.getByRole('button', { name: 'Model', exact: true }).click();
    await settings.getByRole('combobox', { name: 'Model', exact: true }).selectOption('fixture/second-model');
    await settings.getByRole('button', { name: 'Save Root model', exact: true }).click();
    await expect(settings.getByText(/Pending reload/)).toBeVisible();
    await settings.getByRole('button', { name: 'Reload', exact: true }).click();
    await expect(settings.getByRole('alert')).toContainText('Reload busy');
    await expect(settings.getByRole('alert')).toContainText('remains authoritative');
    await settings.getByRole('tab', { name: 'Effective', exact: true }).click();
    await settings.getByRole('button', { name: 'Overview', exact: true }).click();
    await expect(settings.getByRole('region', { name: 'Effective configuration', exact: true })).toContainText('fixture/console-model');
    await settings.getByRole('tab', { name: 'Workspace', exact: true }).click();
    await settings.getByRole('button', { name: 'Model', exact: true }).click();
    await settings.getByRole('button', { name: 'Remove Root model', exact: true }).click();
    await expect(settings.getByText(/Source saved. Use Reload/)).toBeVisible();
    await closeSettings(page); await page.getByRole('tab', { name: 'Chat', exact: true }).click();

    await page.locator(`button[data-session-id="${idB}"]`).click(); await send('Use B while A runs');
    await expect(page.getByText('B stayed responsive.', { exact: true })).toBeVisible();
    // The actual TUI client joins the browser's server. Handoff is explicit;
    // neither client needs a private API or a second writable controller.
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    await expect(remote.attach(idB)).rejects.toMatchObject({ kind: 'controller_in_use' });
    await closeView(idB);
    const terminalB = await remote.attach(idB);
    expect(JSON.stringify(terminalB.state.transcript)).toContain('B stayed responsive.');
    expect((await remote.readSession(idA)).id).toBe(idA);
    await terminalB.resync();
    await remote.detach(idB);
    await page.locator(`button[data-session-id="${idB}"]`).click();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await remote.shutdown(); remote = undefined;

    await page.locator(`button[data-session-id="${idA}"]`).click();
    await connectionAction(page, 'Disconnect');
    await expect(page.getByLabel('Session status')).toContainText(/Connection interrupted|Needs verification/);
    await connectionAction(page, 'Reconnect');
    await expect(page.getByRole('dialog', { name: 'Connection', exact: true })).toHaveCount(0); await showInspector(page);
    await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    await reload(); await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    const incarnationA = JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).runtime_incarnation;
    await closeView(idA);
    await expect(page.locator(`button[data-session-id="${idA}"]`)).toBeVisible();
    await expect(page.locator(`button[data-session-id="${idA}"]`)).toHaveAttribute('title', '');
    // The only controller has been released before the provider finishes A.
    await fixture.release('finish-a');
    await fixture.control('observations/await?kind=response_completed&count=2&timeoutMs=30000');
    await page.locator(`button[data-session-id="${idA}"]`).click();
    await expect(page.getByText('A is running. A finished.', { exact: true })).toBeVisible();
    expect(JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).runtime_incarnation).toBe(incarnationA);
    // Alternate more than the native 32-attachment capacity on one connection.
    // Every close is acknowledged as detached, and every reopen keeps residency.
    for (let i = 0; i < 34; i++) {
      const id = i % 2 === 0 ? idA : idB;
      await closeView(id);
      await expect(page.locator(`button[data-session-id="${id}"]`)).toHaveAttribute('title', '');
      await page.locator(`button[data-session-id="${id}"]`).click();
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
      await expect.poll(async () => JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).SessionId).toBe(id);
    }
    await page.locator(`button[data-session-id="${idA}"]`).click();
    await send('Approval please'); await expect(page.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    const pendingApproval = JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).pending_interactions[0].interaction;
    await connectionAction(page, 'Disconnect'); await reload();
    await expect(page.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    expect(JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).pending_interactions[0].interaction).toEqual(pendingApproval);
    await page.getByRole('button', { name: 'Allow once' }).click();
    // Observe the real provider continuation before asserting its presentation.
    // Native Tool completion and provider response completion are separate boundaries.
    await fixture.control('observations/await?kind=response_completed&count=4&timeoutMs=30000');
    await expect(page.getByText('Approval completed.', { exact: true })).toBeVisible();
    await send('Questionnaire please'); await expect(page.getByRole('region', { name: 'Questionnaire' })).toBeVisible();
    await reload(); await page.getByRole('radio', { name: 'Keep native', exact: true }).click(); await page.getByRole('button', { name: 'Submit answers' }).click();
    await expect(page.getByText('Questionnaire completed.', { exact: true })).toBeVisible();
    await send('Publish while detached'); await fixture.gate('publish-question');
    await connectionAction(page, 'Disconnect');
    await expect(page.getByLabel('Session status')).toContainText(/Connection interrupted|Needs verification/);
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    // Observe connection cleanup at the native owner before publishing an
    // interaction with no UI attached. This observer never attaches a Session.
    await expect.poll(async () => (await remote!.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.external_attachments).toBe(0);
    await fixture.release('publish-question');
    await fixture.control('observations/await?kind=response_completed&count=7&timeoutMs=30000');
    await remote.shutdown(); remote = undefined;
    await connectionAction(page, 'Reconnect');
    await expect(page.getByRole('dialog', { name: 'Connection', exact: true })).toHaveCount(0); await showInspector(page);
    await expect(page.getByRole('region', { name: 'Questionnaire' })).toBeVisible();
    await page.getByRole('radio', { name: 'Keep native', exact: true }).click(); await page.getByRole('button', { name: 'Submit answers' }).click();
    await expect(page.getByText('Detached question completed.', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Native diagnostic JSON')).toContainText('console-model');
    fixture.writeSettings('second-model');
    await page.locator(`button[data-session-id="${idA}"]`).click();
    await expect(page.getByLabel('Native diagnostic JSON')).toContainText('console-model');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('button', { name: 'Reload', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Configuration published' })).toBeVisible();
    await closeSettings(page);
    await expect(page.getByLabel('Native diagnostic JSON')).toContainText('second-model');
    await expect(page.getByText('A is running. A finished.', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Session location', { exact: true })).toContainText(fixture.workspaceA);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('second-model');
    await page.getByLabel('Method filter').fill('session/attach');
    await expect(page.locator('.protocol-log')).toContainText('session/attach');
    await page.getByRole('button', { name: 'Pause log' }).click(); await expect(page.getByRole('button', { name: 'Resume log' })).toBeVisible();
    await page.getByRole('button', { name: 'Resume log' }).click();
    await page.screenshot({ path: 'test-results/console-desktop.png', fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 }); await page.screenshot({ path: 'test-results/console-mobile.png', fullPage: true });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(fixture.token);
    expect(await page.locator('body').innerText()).not.toContain('fake-provider-only');
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.locator(`button[data-session-id="${idB}"]`).click();
    await page.locator(`button[data-session-id="${idB}"]`).hover();
    await page.locator(`button[data-session-actions="${idB}"]`).click();
    await page.getByRole('menuitem', { name: 'Delete Session', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Confirm Session deletion' })).toBeVisible();
    await page.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Session deleted.');
    await expect(page.locator(`button[data-session-id="${idB}"]`)).toHaveCount(0);
    await expect(page.getByLabel('Session title')).toHaveText('Long action in A');
    await expect.poll(async () => JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).SessionId).toBe(idA);
    expect(readFileSync(`${fixture.workspaceA}/console-effect`, 'utf8')).toBe('x');
    expect((await fixture.control('requests')).requests).toHaveLength(8);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics(), await page.locator('.notice').allTextContents(), await page.locator('.protocol-log pre').allTextContents()); throw error; }
  finally { await remote?.shutdown(); await page.close(); await fixture.stop(passed); }
});
