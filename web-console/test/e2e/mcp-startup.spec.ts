import { expect, test } from '@playwright/test';
import { appendFileSync, renameSync, symlinkSync } from 'node:fs';
import { join } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { mcpStartupGate, startupToolSchema, startupWait } from '../../../tui/test/support/mcp-startup-gate';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { chooseWorkspace, connectRemote } from './shell-actions';
import { wireProbe } from './wire-probe';

test('Web first Send waits for live MCP discovery and the first provider request has the exact tools', async ({ page }) => {
  const fixture = await startDogfood('mcp_startup');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser test retired')));
  let ownedGate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let passed = false;
  try {
    const gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    ownedGate = gate;
    const wire = await wireProbe(page);
    wire.holdNext('session/create');
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('mcp startup from the Web');
    expect(wire.requests.filter(row => row.method === 'session/create')).toHaveLength(0);
    expect(wire.requests.filter(row => row.method === 'session/submissionIntent')).toHaveLength(0);
    expect(gate.calls).toBe(0);
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.held()).toBe(1);
    // Naming this test Session prevents a separate Title model request.
    const native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    try { await native.renameSession(wire.responses.find(row => row.method === 'session/create')!.result.session.id, 'MCP startup'); }
    finally { await native.shutdown(); }
    wire.releaseHeld();
    await startupWait(() => gate.reached, cancellation.signal);
    const preparation = wire.requests.filter(row => ['session/create', 'session/submissionIntent', 'session/attach'].includes(row.method));
    expect(preparation.map(row => row.method)).toEqual(['session/create', 'session/submissionIntent', 'session/attach']);
    await expect(page.locator('[data-first-submission]')).toHaveAttribute('data-first-submission', 'attaching');
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(0);
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    gate.release();
    await fixture.gate('startup-request');
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(1);
    expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
    expect(requests[0].body.tools[0].function.parameters).toEqual(startupToolSchema);
    expect(gate.calls).toBe(1);
    expect(wire.requests.filter(row => row.method === 'session/create')).toHaveLength(1);
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(1);
    await fixture.release('startup-request');
    await expect(page.getByText('Ready tools received.', { exact: true })).toBeVisible();
    passed = true;
  } catch (error) {
    console.error(fixture.diagnostics());
    throw error;
  } finally {
    cancellation.abort(new Error('Browser test settled'));
    try { await ownedGate?.close(); }
    finally { await fixture.stop(passed); }
  }
});

test('Workspace directory replacement before create ACK sends no submission intent and starts no MCP runtime', async ({ page }) => {
  const fixture = await startDogfood('mcp_submission_refused');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser test retired')));
  let ownedGate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined;
  let passed = false;
  try {
    const gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    ownedGate = gate;
    const wire = await wireProbe(page);
    wire.holdNext('session/create');
    const activeNative = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    native = activeNative;
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('refused submission');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.held()).toBe(1);
    // Keep the MCP document and peer valid behind the replacement. Only fresh
    // Product Host authorization prevents native initialization of this cwd.
    const moved = join(fixture.directory, 'replaced-workspace');
    renameSync(fixture.workspaceA, moved);
    symlinkSync(moved, fixture.workspaceA);
    wire.releaseHeld();
    await expect(page.getByRole('tabpanel', { name: 'Chat', exact: true }).getByRole('alert')).toContainText('not authorized by this Product Host');
    expect(wire.requests.filter(row => ['session/submissionIntent', 'session/attach', 'turn/start'].includes(row.method))).toHaveLength(0);
    const diagnostic = (await activeNative.client.call('server/diagnostics', {}, 'diagnostics')).snapshot;
    expect(diagnostic.loading).toBe(0);
    expect(diagnostic.loaded).toBe(0);
    expect(diagnostic.sessions).toEqual([]);
    expect(gate.calls).toBe(0);
    expect((await fixture.control('requests')).requests).toEqual([]);
    passed = true;
  } catch (error) {
    console.error(fixture.diagnostics());
    throw error;
  } finally {
    cancellation.abort(new Error('Browser test settled'));
    try { await ownedGate?.close(); }
    finally {
      try { await page.close(); await native?.shutdown(); }
      finally { await fixture.stop(passed); }
    }
  }
});

test('first Send reaches Native and arms its configured budget through six cold attachment waits', async ({ page }) => {
  const fixture = await startDogfood('mcp_startup_timeout');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser test retired')));
  let ownedGate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined;
  let passed = false;
  try {
    const gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    ownedGate = gate;
    appendFileSync(join(fixture.workspaceA, 'rustx.toml'), '\n[mcp_startup_policy]\nwait_timeout_ms = 1000\n');
    const activeNative = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    native = activeNative;
    const sessions = [];
    for (let index = 0; index < 6; index++) {
      const created = await activeNative.createSession({ cwd: fixture.workspaceA });
      await activeNative.renameSession(created.session.id, `Cold MCP ${index}`);
      sessions.push(created.session);
    }
    expect(gate.calls).toBe(0);
    const ids = sessions.map(session => session.id);
    await page.addInitScript(({ endpoint, ids }) => localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ids })), { endpoint: new URL(fixture.endpoint).href, ids });
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    // All six real peers have entered tools/list. No submission has armed any
    // clock, and no response has freed an ordinary client/native request slot.
    await expect.poll(() => gate.calls).toBe(6);
    expect(wire.requests.filter(row => row.method === 'session/attach')).toHaveLength(6);
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(0);
    expect(wire.requests.filter(row => row.method === 'session/submissionIntent')).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('mcp startup under saturated attachments');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.requests.filter(row => row.method === 'session/submissionIntent').length).toBe(1);
    expect(wire.requests.find(row => row.method === 'session/submissionIntent')!.params).toEqual({ session_id: sessions[0].id, node_id: sessions[0].active_node });
    // Never release discovery: only the native configured deadline can settle
    // this selected source. The other five Conversations remain unarmed.
    await fixture.gate('startup-timeout-request');
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(1);
    expect(requests[0].body.tools ?? []).toEqual([]);
    expect(gate.calls).toBe(6);
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(1);
    const intent = wire.responses.findIndex(row => row.method === 'session/submissionIntent');
    const attached = wire.responses.findIndex(row => row.method === 'session/attach');
    expect(intent).toBeGreaterThanOrEqual(0);
    expect(intent).toBeLessThan(attached);
    await fixture.release('startup-timeout-request');
    await expect(page.getByText('Submission deadline applied.', { exact: true })).toBeVisible();
    passed = true;
  } catch (error) {
    console.error(fixture.diagnostics());
    throw error;
  } finally {
    cancellation.abort(new Error('Browser test settled'));
    try { await ownedGate?.close(); }
    finally {
      try { await page.close(); await native?.shutdown(); }
      finally { await fixture.stop(passed); }
    }
  }
});
