import { expect, test } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { closeSettings, connectRemote, openSettingsPage, openWorkspaceSettings } from './shell-actions';
import { wireProbe } from './wire-probe';
import type { TraceRequestDetail } from '../../../protocol/app-server/v21';

function immutable(request: TraceRequestDetail) {
  const { usage: _usage, failure: _failure, generation: _generation, ...input } = request;
  return input;
}

test('T1-17 X03 X04 X05 X06 X09 Settings save/reread, busy gate, exact adoption and historical Trace', async ({ page }) => {
  const f = await startDogfood('web_trace_convergence');
  const remote = await AppServerHost.connectRemote({ endpoint: f.endpoint, token: f.token });
  const wire = await wireProbe(page);
  let passed = false;
  try {
    const a = (await remote.client.call('session/create', { settings: { cwd: f.workspaceA } }, 'session_transition')).session.id;
    const b = (await remote.client.call('session/create', { settings: { cwd: f.workspaceB } }, 'session_transition')).session.id;
    const attachedA = await remote.client.call('session/attach', { session_id: a }, 'attached');
    const attachedB = await remote.client.call('session/attach', { session_id: b }, 'attached');
    const bindingB = (await remote.client.call('session/effectiveConfiguration', { target: attachedB.target }, 'effective_configuration')).projection.adopted_binding;
    const trace = async () => (await remote.client.call('session/trace', { target: attachedA.target, limit: 32 }, 'trace')).page;
    const detail = async (record_id: string) => (await remote.client.call('session/traceDetail', { target: attachedA.target, record_id }, 'trace_detail')).detail!.request!;
    await remote.client.call('turn/start', { target: attachedA.target, content: [{ type: 'text', text: 'Before adoption' }] }, 'inbound_accepted');
    await f.gate('trace-before');
    const oldRecord = (await trace()).records.find(record => record.kind === 'request')!;
    const before = immutable(await detail(oldRecord.id));
    expect(before.tools.some(tool => tool.name === 'read')).toBe(true);
    expect(before.effective_system_prompt.text).not.toContain('TRACE_NEW_INSTRUCTIONS');
    // Consumer B cannot prepare this source generation; it must retain its own
    // independent failure and adopted binding while A prepares successfully.
    writeFileSync(join(f.workspaceB, 'rustx.toml'), '[invalid TOML');
    await routeWorkspaceHost(page, f); await page.goto('/'); await connectRemote(page, f.endpoint, f.token);
    await openWorkspaceSettings(page, 'Workspace A');
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await openSettingsPage(page, 'Agent');
    await settings.getByLabel('Instructions', { exact: true }).fill('TRACE_NEW_INSTRUCTIONS');
    await settings.getByRole('button', { name: 'Save Root instructions', exact: true }).click();
    await expect(settings.getByText('Root instructions saved. Native coordination owns application.')).toBeVisible();
    await closeSettings(page);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await openSettingsPage(page, 'Tools & Permissions');
    await settings.getByLabel('read', { exact: true }).uncheck();
    await settings.getByRole('button', { name: 'Save Native Tools', exact: true }).click();
    await expect(settings.getByText('Native Tools saved. Native coordination owns application.')).toBeVisible();
    const source = await remote.client.call('configuration/sourcesRead', { target: { kind: 'user' } }, 'source_settings');
    const workspaceA = (await f.workspaceHost.host.listWorkspaces()).workspaces.find(workspace => workspace.displayName === 'Workspace A')!;
    const workspaceSource = await f.workspaceHost.host.configureWorkspace(workspaceA.id, f.endpoint, { kind: 'read' });
    if (workspaceSource.kind !== 'read') throw new Error('Expected native Workspace reread');
    expect(workspaceSource.projection.workspace?.authored?.agent?.instructions).toBe('TRACE_NEW_INSTRUCTIONS');
    expect(source.projection.user.authored?.agent?.tools?.builtin).not.toContain('read');
    const application = async (id: string) => (await remote.client.call('session/configuration', { session_id: id }, 'session_configuration')).application!;
    await expect.poll(async () => (await application(a)).candidate != null).toBe(true);
    const busy = await application(a);
    expect(busy.eligibility.status).toBe('busy');
    expect(immutable(await detail(oldRecord.id))).toEqual(before);
    await expect(remote.client.call('session/adoptConfiguration', { session_id: a, candidate: busy.candidate!.identity, expected_binding: busy.candidate!.expected_binding }, 'configuration_application')).rejects.toThrow();
    expect((await f.control('requests')).requests).toHaveLength(1);
    await f.release('trace-before');
    await expect.poll(async () => (await application(a)).eligibility.status).toBe('eligible');
    const eligible = await application(a);
    await remote.client.call('session/adoptConfiguration', { session_id: a, candidate: eligible.candidate!.identity, expected_binding: eligible.candidate!.expected_binding }, 'configuration_application');
    expect(immutable(await detail(oldRecord.id))).toEqual(before);
    const afterB = await remote.client.call('session/effectiveConfiguration', { target: attachedB.target }, 'effective_configuration');
    expect(afterB.projection.adopted_binding).toBe(bindingB);
    await expect.poll(async () => Object.values((await application(b)).units).some(unit => unit.status === 'failed')).toBe(true);
    expect((await application(a)).candidate).toBeNull();
    await remote.client.call('turn/start', { target: attachedA.target, content: [{ type: 'text', text: 'After adoption' }] }, 'inbound_accepted');
    await expect.poll(async () => (await trace()).records.filter(record => record.kind === 'request' && record.state === 'completed').length).toBe(2);
    const current = (await trace()).records.filter(record => record.kind === 'request').at(-1)!;
    expect(current.request!.request_id).not.toBe(before.request_id);
    expect(current.request!.predecessor).toEqual({ availability: 'available', request_id: before.request_id });
    expect(current.request!.system_prompt.state).toBe('changed'); expect(current.request!.tool_catalog).toBe('changed');
    const after = await detail(current.id);
    expect(after.effective_system_prompt.text).toContain('TRACE_NEW_INSTRUCTIONS');
    expect(after.tools.some(tool => tool.name === 'read')).toBe(false);
    expect(after.previous_system_prompt).toEqual(before.effective_system_prompt);
    expect(immutable(await detail(oldRecord.id))).toEqual(before);
    expect((await f.control('requests')).requests).toHaveLength(2);
    expect(wire.requests.filter(request => request.method === 'configuration/sourceWrite')).toHaveLength(1);
    expect(wire.requests.filter(request => request.method === 'turn/start')).toHaveLength(0);
    await closeSettings(page);
    await remote.client.call('session/detach', { target: attachedA.target }, 'detached');
    await page.locator(`button[data-session-id="${a}"]`).click();
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    const boundary = page.locator(`[data-display-type="SystemRow"][data-owner="${current.id}"]`);
    await expect(boundary).toContainText('System Prompt and Tools Updated');
    await boundary.click();
    await page.getByRole('tab', { name: 'Diff', exact: true }).click();
    await expect(page.getByRole('complementary', { name: 'Trace record inspector' }).getByRole('tabpanel')).toContainText('TRACE_NEW_INSTRUCTIONS');
    await page.screenshot({ path: 'test-results/trajectory-native-adoption.png' });
    passed = true;
  } finally { await remote.shutdown(); await page.close(); await f.stop(passed); }
});
