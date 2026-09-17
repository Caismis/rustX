import { routeWorkspaceHost } from './workspace-host';
import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host.ts';
import { startDogfood } from './dogfood-server';

test('two real rustX Sessions, browser loss, native interactions, raw wire, and cold configuration resolution', async ({ page }) => {
  const fixture = await startDogfood();
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const connect = async () => {
    await page.getByLabel('WebSocket endpoint').fill(`${fixture.endpoint}/`);
    await page.getByLabel('Transport token').fill(fixture.token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
  };
  const send = async (text: string) => { await page.getByRole('textbox', { name: 'Message', exact: true }).fill(text); await page.getByRole('button', { name: 'Send', exact: true }).click(); };
  const reload = async () => { await page.reload(); await connect(); };
  let remote: AppServerHost | undefined;
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await expect(page).toHaveTitle('rustX Developer Console');
    await expect(page.getByRole('heading', { name: 'Sessions, in motion.' })).toBeVisible();
    await connect();
    await page.getByLabel('Choose Workspace').selectOption({ label: 'Workspace A' });
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toHaveText(`${fixture.workspaceA} · attached`);
    const idA = await page.locator('.session-toolbar strong').innerText();
    await page.getByLabel('Choose Workspace').selectOption({ label: 'Workspace B' }); await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toHaveText(`${fixture.workspaceB} · attached`);
    const idB = await page.locator('.session-toolbar strong').innerText();
    await expect(page.getByRole('tab', { name: /^ses_/ })).toHaveCount(2);
    await page.getByRole('tab', { name: idA, exact: true }).click();
    await send('Long action in A'); await fixture.gate('finish-a');
    await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    // Configuration source commits cannot rewrite a running admitted attempt.
    await page.getByRole('tab', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
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
    await expect(settings.getByText(/Source saved. The loaded runtime/)).toBeVisible();
    await page.getByRole('tab', { name: 'Chat', exact: true }).click();

    await page.getByRole('tab', { name: idB, exact: true }).click(); await send('Use B while A runs');
    await expect(page.getByText('B stayed responsive.', { exact: true })).toBeVisible();
    // The actual TUI client joins the browser's server. Handoff is explicit;
    // neither client needs a private API or a second writable controller.
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    await expect(remote.attach(idB)).rejects.toMatchObject({ kind: 'controller_in_use' });
    await page.getByRole('button', { name: 'Detach', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('detached');
    const terminalB = await remote.attach(idB);
    expect(JSON.stringify(terminalB.state.transcript)).toContain('B stayed responsive.');
    expect((await remote.readSession(idA)).id).toBe(idA);
    await terminalB.resync();
    await remote.detach(idB);
    await page.getByRole('button', { name: 'Attach / cold resume' }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('attached');
    await remote.shutdown(); remote = undefined;

    await page.getByRole('tab', { name: idA, exact: true }).click();
    await page.getByRole('button', { name: 'Disconnect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('disconnected');
    await page.getByRole('button', { name: 'Reconnect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
    await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    await reload(); await expect(page.getByText('A is running.', { exact: true })).toBeVisible();
    const incarnationA = JSON.parse(await page.getByLabel('Runtime facts').innerText()).runtime_incarnation;
    await page.getByRole('button', { name: `Close view ${idA}` }).click();
    await expect(page.getByRole('tab', { name: idA, exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: `Open ${idA}`, exact: true })).toContainText('detached');
    // The only controller has been released before the provider finishes A.
    await fixture.release('finish-a');
    await fixture.control('observations/await?kind=response_completed&count=2&timeoutMs=30000');
    await page.getByRole('button', { name: `Open ${idA}`, exact: true }).click();
    await expect(page.getByText('A is running. A finished.', { exact: true })).toBeVisible();
    expect(JSON.parse(await page.getByLabel('Runtime facts').innerText()).runtime_incarnation).toBe(incarnationA);
    // Alternate more than the native 32-attachment capacity on one connection.
    // Every close is acknowledged as detached, and every reopen keeps residency.
    for (let i = 0; i < 34; i++) {
      const id = i % 2 === 0 ? idA : idB;
      await page.getByRole('button', { name: `Close view ${id}` }).click();
      await expect(page.getByRole('button', { name: `Open ${id}`, exact: true })).toContainText('detached');
      await page.getByRole('button', { name: `Open ${id}`, exact: true }).click();
      await expect(page.locator('.session-toolbar small')).toContainText('attached');
      await expect(page.locator('.session-toolbar strong')).toHaveText(id);
    }
    await page.getByRole('tab', { name: idA, exact: true }).click();
    await send('Approval please'); await expect(page.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    const pendingApproval = JSON.parse(await page.getByLabel('Runtime facts').innerText()).pending_interactions[0].interaction;
    await page.getByRole('button', { name: 'Disconnect', exact: true }).click(); await reload();
    await expect(page.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    expect(JSON.parse(await page.getByLabel('Runtime facts').innerText()).pending_interactions[0].interaction).toEqual(pendingApproval);
    await page.getByRole('button', { name: 'Allow once' }).click();
    // Observe the real provider continuation before asserting its presentation.
    // Native Tool completion and provider response completion are separate boundaries.
    await fixture.control('observations/await?kind=response_completed&count=4&timeoutMs=30000');
    await expect(page.getByText('Approval completed.', { exact: true })).toBeVisible();
    await send('Questionnaire please'); await expect(page.getByRole('region', { name: 'Questionnaire' })).toBeVisible();
    await reload(); await page.getByRole('radio', { name: 'Keep native', exact: true }).click(); await page.getByRole('button', { name: 'Submit answers' }).click();
    await expect(page.getByText('Questionnaire completed.', { exact: true })).toBeVisible();
    await send('Publish while detached'); await fixture.gate('publish-question');
    await page.getByRole('button', { name: 'Disconnect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('disconnected');
    remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    // Observe connection cleanup at the native owner before publishing an
    // interaction with no UI attached. This observer never attaches a Session.
    await expect.poll(async () => (await remote!.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.external_attachments).toBe(0);
    await fixture.release('publish-question');
    await fixture.control('observations/await?kind=response_completed&count=7&timeoutMs=30000');
    await remote.shutdown(); remote = undefined;
    await page.getByRole('button', { name: 'Reconnect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
    await expect(page.getByRole('region', { name: 'Questionnaire' })).toBeVisible();
    await page.getByRole('radio', { name: 'Keep native', exact: true }).click(); await page.getByRole('button', { name: 'Submit answers' }).click();
    await expect(page.getByText('Detached question completed.', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Runtime facts')).toContainText('console-model');
    fixture.writeSettings('second-model');
    await page.getByRole('button', { name: 'Resync', exact: true }).click();
    await expect(page.getByLabel('Runtime facts')).toContainText('console-model');
    await page.getByRole('button', { name: 'Unload runtime', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('unloaded');
    await page.getByRole('button', { name: 'Attach / cold resume' }).click();
    await expect(page.getByLabel('Runtime facts')).toContainText('second-model');
    await expect(page.getByText('A is running. A finished.', { exact: true })).toBeVisible();
    await expect(page.locator('.session-toolbar small')).toContainText(fixture.workspaceA);
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
    await page.getByRole('tab', { name: idB, exact: true }).click();
    await page.getByRole('button', { name: 'Unload runtime', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('unloaded');
    await page.getByLabel(`Actions ${idB}`, { exact: true }).click();
    await page.getByRole('button', { name: `Delete ${idB}`, exact: true }).click();
    await expect(page.getByRole('region', { name: 'Confirm Session deletion' })).toBeVisible();
    await page.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('"status": "deleted"');
    await expect(page.getByRole('button', { name: `Open ${idB}`, exact: true })).toHaveCount(0);
    await expect(page.getByRole('tab', { name: /^ses_/ })).toHaveCount(1);
    await expect(page.locator('.session-toolbar strong')).toHaveText(idA);
    expect(readFileSync(`${fixture.workspaceA}/console-effect`, 'utf8')).toBe('x');
    expect((await fixture.control('requests')).requests).toHaveLength(8);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics(), await page.locator('.notice').allTextContents(), await page.locator('.protocol-log pre').allTextContents()); throw error; }
  finally { await remote?.shutdown(); await page.close(); await fixture.stop(passed); }
});
