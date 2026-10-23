import { selectedSessionId, selectedTarget, selectedCheckpoint } from './shell-actions';
import { openQuestionnaire } from './source-reading';
import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import { connectRemote } from './shell-actions';
import { closeSessionView } from './shell-actions';
import {
  choose, closeSettings, confirmSettingsAction, connectionAction, openSettingsPage, openWorkspaceSettings, selectedSettingsPage,
} from './shell-actions';
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
    await connectRemote(page, `${fixture.endpoint}/`, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
  };
  const openSession = async (id: string) => {
    await page.locator(`button[data-session-id="${id}"]`).click();
    await expect(page.locator(`button[data-session-id="${id}"]`)).toHaveAttribute('aria-current', 'page');
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
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
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
    const idA = await selectedSessionId(page);
    await openEmptySession(page, fixture, 'Workspace B');
    await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
    const idB = await selectedSessionId(page);
    await expect(page.getByRole('tree', { name: 'Session browser' })).toHaveCount(1);
    await expect(page.getByRole('tablist', { name: 'Open Session views' })).toHaveCount(0);
    await openSession(idA);
    await send('Long action in A'); await fixture.gate('finish-a');
    await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    // Configuration source commits cannot rewrite a running admitted attempt.
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    expect(await selectedSettingsPage(page)).toBe('General');
    await closeSettings(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    const defaultModel = settings.getByRole('form', { name: 'Default model', exact: true });
    await choose(defaultModel, 'Model', 'fixture/second-model');
    await defaultModel.getByRole('button', { name: 'Save Default model', exact: true }).click();
    await expect(settings.getByText('Default model saved.')).toBeVisible();
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    await expect(settings.getByRole('button', { name: /Adopt/ })).toHaveCount(0);
    await openSettingsPage(page, 'Models'); await expandModelAuthoring(page);
    await confirmSettingsAction(page, 'Use global default Default model');
    await expect(settings.getByText('Default model saved.')).toBeVisible();
    await closeSettings(page); await page.getByRole('tab', { name: 'Chat', exact: true }).click();

    await openSession(idB); await send('Use B while A runs');
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
    await openSession(idB);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await remote.shutdown(); remote = undefined;

    await openSession(idA);
    await connectionAction(page, 'Disconnect');
    await expect(page.locator('button[data-phase=disconnected]')).toBeVisible();
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    await reload(); await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    await expect.poll(() => selectedTarget(page, wire)).toMatchObject({
      session_id: idA, runtime_incarnation: expect.any(String),
    });
    const incarnationA = (await selectedTarget(page, wire)).runtime_incarnation;
    await closeView(idA);
    await expect(page.locator(`button[data-session-id="${idA}"]`)).toBeVisible();
    await expect(page.locator(`button[data-session-id="${idA}"]`)).toHaveAttribute('title', '');
    // The only controller has been released before the provider finishes A.
    await fixture.release('finish-a');
    await fixture.control('observations/await?kind=response_completed&count=2&timeoutMs=30000');
    await openSession(idA);
    await expect(page.getByText('A is running. A finished.', { exact: true })).toBeVisible();
    // Cached transcript text can paint before the native attachment completes.
    await expect.poll(() => selectedTarget(page, wire)).toMatchObject({
      session_id: idA, runtime_incarnation: incarnationA,
    });
    // Alternate more than the native 32-attachment capacity on one connection.
    // Every close is acknowledged as detached, and every reopen keeps residency.
    for (let i = 0; i < 34; i++) {
      const id = i % 2 === 0 ? idA : idB;
      await closeView(id);
      await expect(page.locator(`button[data-session-id="${id}"]`)).toHaveAttribute('title', '');
      await page.locator(`button[data-session-id="${id}"]`).click();
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
      await expect.poll(() => selectedSessionId(page)).toBe(id);
    }
    await openSession(idA);
    await send('Approval please'); await expect(page.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    const pendingApproval = (await selectedCheckpoint(page, wire)).snapshot.pending_interactions![0].interaction;
    await connectionAction(page, 'Disconnect'); await reload();
    await expect(page.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    expect((await selectedCheckpoint(page, wire)).snapshot.pending_interactions![0].interaction).toEqual(pendingApproval);
    await page.getByRole('button', { name: 'Allow once' }).click();
    // Observe the real provider continuation before asserting its presentation.
    // Native Tool completion and provider response completion are separate boundaries.
    await fixture.control('observations/await?kind=response_completed&count=4&timeoutMs=30000');
    await expect(page.getByText('Approval completed.', { exact: true })).toBeVisible();
    await send('Questionnaire please'); await openQuestionnaire(page);
    await reload(); await openQuestionnaire(page); await page.getByRole('radio', { name: 'Keep native', exact: true }).click(); await page.getByRole('button', { name: 'Submit', exact: true }).click();
    await expect(page.getByText('Questionnaire completed.', { exact: true })).toBeVisible();
    await send('Publish while detached'); await fixture.gate('publish-question');
    await connectionAction(page, 'Disconnect');
    await expect(page.locator('button[data-phase=disconnected]')).toBeVisible();
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    // Observe connection cleanup at the native owner before publishing an
    // interaction with no UI attached. This observer never attaches a Session.
    await expect.poll(async () => (await remote!.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.external_attachments).toBe(0);
    await fixture.release('publish-question');
    await fixture.control('observations/await?kind=response_completed&count=7&timeoutMs=30000');
    await remote.shutdown(); remote = undefined;
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await openQuestionnaire(page);
    await page.getByRole('radio', { name: 'Keep native', exact: true }).click(); await page.getByRole('button', { name: 'Submit', exact: true }).click();
    await expect(page.getByText('Detached question completed.', { exact: true })).toBeVisible();
    await expect.poll(async () => (await selectedCheckpoint(page, wire)).snapshot.model?.configured.model).toBe('fixture/console-model');
    fixture.writeSettings('second-model');
    await openSession(idA);
    await expect.poll(async () => (await selectedCheckpoint(page, wire)).snapshot.model?.configured.model).toBe('fixture/console-model');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    expect(await selectedSettingsPage(page)).toBe('General');
    await openSettingsPage(page, 'Advanced');
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    await closeSettings(page);
    await expect.poll(async () => (await selectedCheckpoint(page, wire)).snapshot.model?.configured.model).toBe('fixture/console-model');
    // The fixed 32-row native tail excludes A's earliest response after the
    // intervening interaction audits. Load its advancing historical page.
    if (await page.getByText('A is running. A finished.', { exact: true }).count() === 0)
      await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByText('A is running. A finished.', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('second-model');
    expect(wire.requests.some(request => request.method === 'session/attach')).toBe(true);
    await page.screenshot({ path: 'test-results/console-desktop.png', fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 }); await page.screenshot({ path: 'test-results/console-mobile.png', fullPage: true });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(fixture.token);
    expect(await page.locator('body').innerText()).not.toContain('fake-provider-only');
    await page.setViewportSize({ width: 1440, height: 1000 });
    await openSession(idB);
    await page.locator(`button[data-session-id="${idB}"]`).hover();
    await page.locator(`button[data-session-actions="${idB}"]`).click();
    await page.getByRole('menuitem', { name: 'Delete Session', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Delete this session?' })).toBeVisible();
    await page.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Delete this session?' })).toHaveCount(0);
    await expect(page.getByText('Session deleted.', { exact: true })).toHaveCount(0);
    await expect(page.locator(`button[data-session-id="${idB}"]`)).toHaveCount(0);
    await expect(page.getByLabel('Session title')).toHaveText('New session');
    await expect.poll(() => selectedSessionId(page)).toBe(idA);
    expect(readFileSync(`${fixture.workspaceA}/console-effect`, 'utf8')).toBe('x');
    expect((await fixture.control('requests')).requests).toHaveLength(8);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics(), await page.locator('.notice').allTextContents(), wire.responses); throw error; }
  finally { await remote?.shutdown(); await page.close(); await fixture.stop(passed); }
});
