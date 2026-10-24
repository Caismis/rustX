import { expect, test } from '@playwright/test';
import { renameSync, symlinkSync } from 'node:fs';
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
