import { readSource } from './source-reading';
import { openEmptySession } from './shell-actions';
import { expect, test } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, connectionAction, returnHome, openSettingsPage, openWorkspaceSettings } from './shell-actions';
import { wireProbe } from './wire-probe';

test('C01 C02 C06 C08 C09 C10 real zero-Session Settings, Workspace authorization and lost-write recovery', async ({ page }) => {
 const f = await startDogfood();
 const remote = await AppServerHost.connectRemote({ endpoint: f.endpoint, token: f.token });
 const wire = await wireProbe(page);
 try {
   await routeWorkspaceHost(page, f); await page.goto('/'); await connectRemote(page, f.endpoint, f.token);
   const catalog = await f.workspaceHost.host.listWorkspaces();
   const [a, b] = catalog.workspaces;
   const sessions = () => remote.client.call('session/list', { offset: 0, limit: 32 }, 'sessions');
   expect((await sessions()).sessions).toEqual([]);
   await page.getByRole('button', { name: 'Settings', exact: true }).click();
   const settings = page.getByRole('region', { name: 'Settings', exact: true });
   await openSettingsPage(page, 'Advanced');
   await expect(settings.locator('[data-lifecycle=ready]')).toBeVisible();
   await settings.getByLabel('Reserved tokens', { exact: true }).fill('19');
   await settings.getByRole('button', { name: 'Save Context', exact: true }).click();
   await expect(settings.getByText('Context saved.')).toBeVisible();
   const source = await readSource(remote,{kind:'user'});
   expect(source.user.authored?.context?.reserve_tokens).toBe('19');
   expect(source.workspace).toBeNull();
   await returnHome(page);
   await openWorkspaceSettings(page, a.displayName);
   await openSettingsPage(page, 'Tools & Permissions');
   // The User source authors every Native Tool, so this Workspace displays the
   // native effective value while authoring none of it.
   await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
   await expect(settings.getByRole('form', { name: 'Built-in tools', exact: true })).toHaveAttribute('data-authored', 'absent');
   await expect(settings.getByRole('button', { name: 'Save Built-in tools', exact: true })).toBeDisabled();
   // An explicit edit against that displayed value is Workspace A's own draft.
   await settings.getByLabel('read', { exact: true }).uncheck();
   await returnHome(page);
   await openWorkspaceSettings(page, b.displayName);
   await openSettingsPage(page, 'Tools & Permissions');
   // Workspace B is a different target: it still shows the inherited value and
   // never receives Workspace A's draft.
   await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
   await expect(settings.getByRole('button', { name: 'Save Built-in tools', exact: true })).toBeDisabled();
   await settings.getByLabel('write', { exact: true }).uncheck();
   await returnHome(page);
   await openWorkspaceSettings(page, a.displayName);
   await openSettingsPage(page, 'Tools & Permissions');
   await expect(settings.getByLabel('read', { exact: true })).not.toBeChecked();
   await expect(settings.getByLabel('write', { exact: true })).toBeChecked();
   await settings.getByRole('button', { name: 'Save Built-in tools', exact: true }).click();
   await expect(settings.getByText('Built-in tools saved.')).toBeVisible();
   const authored = await f.workspaceHost.host.configureWorkspace(a.id, f.endpoint, { kind: 'read' });
   if (authored.kind !== 'read') throw new Error('expected a read outcome');
   expect(authored.projection.workspace?.authored?.agent?.tools?.builtin).not.toContain('read');
   expect(authored.projection.workspace?.authored?.agent?.tools?.builtin).toContain('write');
   expect((await sessions()).sessions).toEqual([]);
   expect(wire.requests.some(row => ['thread/start', 'thread/resume', 'inbound/queue'].includes(row.method))).toBe(false);
   await expect(f.workspaceHost.host.configureWorkspace('unregistered', f.endpoint, { kind: 'read' })).rejects.toThrow('Unknown');
   await f.workspaceHost.host.removeWorkspace(await f.workspaceHost.host.listWorkspaces(), a.id);
   // A revoked target fences the next authored change and retains the draft.
   await settings.getByLabel('read', { exact: true }).check();
   await settings.getByRole('button', { name: 'Save Built-in tools', exact: true }).click();
   await expect(settings.getByRole('alert').filter({ hasText: /was not saved\. WorkspaceHostError: Workspace Host: Error: Unknown/ })).toBeVisible();
   await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
   await returnHome(page);
   await page.getByRole('button', { name: 'Settings', exact: true }).click();
   await settings.getByRole('button', { name: 'Open User Settings', exact: true }).click();
   await openSettingsPage(page, 'Tools & Permissions');
   // A fresh User lifetime carries no draft, so there is nothing to save until
   // an explicit edit changes the authored value.
   await expect(settings.getByRole('button', { name: 'Save Built-in tools', exact: true })).toBeDisabled();
   await settings.getByLabel('read', { exact: true }).uncheck();
   await expect(settings.getByRole('button', { name: 'Save Built-in tools', exact: true })).toBeEnabled();
   const before = wire.requests.filter(row => row.method === 'configuration/sourceWrite').length;
   wire.loseNext('configuration/sourceWrite');
   await settings.getByRole('button', { name: 'Save Built-in tools', exact: true }).click();
   await expect.poll(wire.lost).toBe(1);
   await connectionAction(page, 'Reconnect');
   await expect(settings.getByRole('button', { name: 'Save Built-in tools', exact: true })).toBeEnabled();
   expect(wire.lost()).toBe(1);
   expect(wire.requests.filter(row => row.method === 'configuration/sourceWrite')).toHaveLength(before + 1);
   expect((await sessions()).sessions).toEqual([]);
 } finally { await remote.shutdown(); const report = await f.stop(false); expect(report.requestCount).toBe(0); }
});

test('C13 C15 real native automatic commit waits for Session work and preserves the settled history', async ({ page }) => {
 const f = await startDogfood(); const wire = await wireProbe(page);
 const remote = await AppServerHost.connectRemote({ endpoint: f.endpoint, token: f.token });
 try {
   await routeWorkspaceHost(page, f); await page.goto('/'); await connectRemote(page, f.endpoint, f.token);
   await openEmptySession(page, f, 'Workspace A');
   const message = page.getByRole('textbox', { name: 'Message', exact: true }); await expect(message).toBeEnabled();
   const id = (await remote.client.call('session/list', { offset: 0, limit: 32 }, 'sessions')).sessions[0].id;
   const selection = await remote.client.call('session/settings', { session_id: id }, 'settings');
   await message.fill('Long action in A'); await message.press('Enter'); await f.gate('finish-a');
   const source = await readSource(remote,{kind:'user'});
   await remote.client.call('configuration/sourceWrite', { target: { kind: 'user' }, expected_revision: source.user.revision,
     mutation: { kind: 'config', mutation: { unit: 'instructions', authored: 'New instructions apply at the next safe boundary.' } } }, 'sources_written');
   const waiting = page.getByText('Configuration changes will apply automatically after the current work settles.');
   await expect(waiting).toBeVisible();
   await page.screenshot({ path: '/tmp/rustx-configuration-waiting.png' });
   // The settled historical Request is frozen across the automatic commit.
   await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
   const requestBoundary = page.locator('[data-request-owner]').first();
   const recordId = await requestBoundary.getAttribute('data-request-owner');
   await requestBoundary.click();
   const traceDomain = `trace.records.${recordId!.slice('trace:'.length)}.detail`;
   const traceResponses = () => wire.responses.filter(row => row.method === 'session/traceDetail' && row.result?.type === 'domain_page' && row.result.page.domain === traceDomain);
   const readTrace = () => {
     const request = wire.requests.filter(row => row.method === 'session/traceDetail' && row.params.record_id === recordId).at(-1)!;
     return wire.traceDetail(request.params.target, traceResponses().at(-1)!.result.page);
   };
   await expect.poll(() => traceResponses().length).toBe(1);
   const oldTrace = await readTrace();
   expect(oldTrace?.id).toBe(recordId);
   await f.release('finish-a');
   // Success is silent: the line disappears once native commits.
   await expect(waiting).toHaveCount(0);
   await expect(page.getByLabel('Session configuration', { exact: true })).toHaveCount(0);
   expect(await remote.client.call('session/settings', { session_id: id }, 'settings')).toEqual(selection);
   expect(await readTrace()).toEqual(oldTrace);
   // An instruction change is Trace-only: Chat shows no row for it.
   await page.getByRole('tab', { name: 'Chat', exact: true }).click();
   await expect(page.locator('[data-configuration-change]')).toHaveCount(0);
   expect((await f.control('requests')).requests).toHaveLength(1);
 } finally { await remote.shutdown(); await page.close(); await f.stop(false); }
});
