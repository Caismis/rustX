import { readApplication, readSource } from './source-reading';
import { openEmptySession } from './shell-actions';
import { expect, test } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, connectionAction, closeSettings, openSettingsPage, openWorkspaceSettings } from './shell-actions';
import { wireProbe } from './wire-probe';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';

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
   const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
   await openSettingsPage(page, 'Advanced');
   await expect(settings.getByText(/Revision:/)).toBeVisible();
   await settings.getByLabel('max_connections', { exact: true }).fill('19');
   await settings.getByRole('button', { name: 'Save App Server policy', exact: true }).click();
   await expect(settings.getByText('App Server policy saved.')).toBeVisible();
   await expect(settings.getByText('Saved process policy is active.')).toBeVisible();
   const source = await readSource(remote,{kind:'user'});
   expect(source.process_bindings?.max_connections).toBe(19);
   expect(source.workspace).toBeNull();
   await closeSettings(page);
   await openWorkspaceSettings(page, a.displayName);
   await openSettingsPage(page, 'Tools & Permissions');
   // The User source authors every Native Tool, so this Workspace displays the
   // native effective value while authoring none of it.
   await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
   await settings.getByRole('form', { name: 'Native Tools', exact: true }).getByRole('button', { name: 'Configuration details', exact: true }).click();
   await expect(settings.getByRole('form', { name: 'Native Tools', exact: true }).getByText('Inherited — no Workspace override')).toBeVisible();
   await expect(settings.getByRole('button', { name: 'Save Native Tools', exact: true })).toBeDisabled();
   // An explicit edit against that displayed value is Workspace A's own draft.
   await settings.getByLabel('read', { exact: true }).uncheck();
   await closeSettings(page);
   await openWorkspaceSettings(page, b.displayName);
   await openSettingsPage(page, 'Tools & Permissions');
   // Workspace B is a different target: it still shows the inherited value and
   // never receives Workspace A's draft.
   await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
   await expect(settings.getByRole('button', { name: 'Save Native Tools', exact: true })).toBeDisabled();
   await settings.getByLabel('write', { exact: true }).uncheck();
   await closeSettings(page);
   await openWorkspaceSettings(page, a.displayName);
   await openSettingsPage(page, 'Tools & Permissions');
   await expect(settings.getByLabel('read', { exact: true })).not.toBeChecked();
   await expect(settings.getByLabel('write', { exact: true })).toBeChecked();
   await settings.getByRole('button', { name: 'Save Native Tools', exact: true }).click();
   await expect(settings.getByText('Native Tools saved.')).toBeVisible();
   const authored = await f.workspaceHost.host.configureWorkspace(a.id, f.endpoint, { kind: 'read' });
   if (authored.kind !== 'read') throw new Error('expected a read outcome');
   expect(authored.projection.workspace?.authored?.agent?.tools?.builtin).not.toContain('read');
   expect(authored.projection.workspace?.authored?.agent?.tools?.builtin).toContain('write');
   expect((await sessions()).sessions).toEqual([]);
   expect(wire.requests.some(row => ['session/create', 'session/attach', 'turn/start'].includes(row.method))).toBe(false);
   await expect(f.workspaceHost.host.configureWorkspace('unregistered', f.endpoint, { kind: 'read' })).rejects.toThrow('Unknown');
   await f.workspaceHost.host.removeWorkspace(await f.workspaceHost.host.listWorkspaces(), a.id);
   // A revoked target fences the next authored change and retains the draft.
   await settings.getByLabel('read', { exact: true }).check();
   await settings.getByRole('button', { name: 'Save Native Tools', exact: true }).click();
   await expect(settings.getByRole('alert').filter({ hasText: /was not saved\. WorkspaceHostError: Workspace Host: Error: Unknown/ })).toBeVisible();
   await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
   await closeSettings(page);
   await page.getByRole('button', { name: 'Settings', exact: true }).click();
   await openSettingsPage(page, 'Tools & Permissions');
   // A fresh User lifetime carries no draft, so there is nothing to save until
   // an explicit edit changes the authored value.
   await expect(settings.getByRole('button', { name: 'Save Native Tools', exact: true })).toBeDisabled();
   await settings.getByLabel('read', { exact: true }).uncheck();
   await expect(settings.getByRole('button', { name: 'Save Native Tools', exact: true })).toBeEnabled();
   const before = wire.requests.filter(row => row.method === 'configuration/sourceWrite').length;
   wire.loseNext('configuration/sourceWrite');
   await settings.getByRole('button', { name: 'Save Native Tools', exact: true }).click();
   await expect.poll(wire.lost).toBe(1);
   await connectionAction(page, 'Reconnect');
   await expect(settings.getByRole('button', { name: 'Save Native Tools', exact: true })).toBeEnabled();
   expect(wire.lost()).toBe(1);
   expect(wire.requests.filter(row => row.method === 'configuration/sourceWrite')).toHaveLength(before + 1);
   expect((await sessions()).sessions).toEqual([]);
 } finally { await remote.shutdown(); const report = await f.stop(false); expect(report.requestCount).toBe(0); }
});

test('C13 C14 C15 C16 C17 real native Busy gate, candidate fence, live eligibility and lost adoption response', async ({ page }) => {
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
     mutation: { kind: 'config', mutation: { unit: 'instructions', authored: 'New instructions require explicit Session adoption.' } } }, 'sources_written');
   const adopt = page.getByRole('button', { name: 'Adopt configuration', exact: true });
   await expect(adopt).toBeVisible(); await expect(adopt).toBeDisabled();
   await expect(page.getByText('Session work must settle before adoption.')).toBeVisible();
   // The disabled banner above is the runtime's own published Busy eligibility;
   // configuration authority carries the candidate only.
   const app = await readApplication(remote, id);
   const candidate = app.candidate!;
   await expect(remote.client.call('session/adoptConfiguration', { session_id: id, candidate: candidate.identity, expected_binding: candidate.expected_binding }, 'configuration_receipt')).rejects.toThrow();
   await expect(remote.client.call('session/adoptConfiguration', { session_id: id, candidate: { ...candidate.identity, attempt: String(BigInt(candidate.identity.attempt) + 1n) }, expected_binding: candidate.expected_binding }, 'configuration_receipt')).rejects.toThrow();
   await f.release('finish-a'); await expect(adopt).toBeEnabled();
   const history = await remote.readSession(id);
   // X07: hold the exact settled historical Request across a lost adoption reply.
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

   const retainedBoundary = (await requestBoundary.elementHandle())!;
   wire.loseNext('session/adoptConfiguration'); await adopt.click();
   await expect.poll(wire.lost).toBe(1);
   await connectionAction(page, 'Reconnect');
   // Transport readiness can still display the retained old attachment. Wait
   // for its replacement before asking the new reader for the same native row.
   const target = await wire.attachedTarget(id);
   await expect.poll(() => wire.responses.some(row => row.method === 'session/subscribe' && sameTarget(row.result?.target, target))).toBe(true);
   await expect.poll(() => retainedBoundary.evaluate(el => el.isConnected)).toBe(false);
   await expect(adopt).toHaveCount(0);
   expect(wire.lost()).toBe(1); expect(wire.requests.filter(row => row.method === 'session/adoptConfiguration')).toHaveLength(1);
   expect((await remote.client.call('session/configuration', { session_id: id }, 'session_configuration')).projection!.inline?.candidate).toBeNull();
   expect(await remote.client.call('session/settings', { session_id: id }, 'settings')).toEqual(selection);
   expect(await remote.readSession(id)).toEqual(history);
   await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
   await page.locator(`[data-request-owner="${recordId}"]`).click();
   await expect.poll(() => traceResponses().length).toBe(2);
   expect(await readTrace()).toEqual(oldTrace);
   expect((await f.control('requests')).requests).toHaveLength(1);
 } finally { await remote.shutdown(); await page.close(); await f.stop(false); }
});
