import { expect, test } from '@playwright/test';
import { appendFileSync, renameSync, symlinkSync } from 'node:fs';
import { join } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { mcpStartupGate, startupToolSchema, startupWait } from '../../../tui/test/support/mcp-startup-gate';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { chooseWorkspace, connectRemote, connectionAction } from './shell-actions';
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

for (const destination of ['existing', 'new'] as const) test(`first Send to ${destination === 'existing' ? 'an existing' : 'a new'} Conversation reaches Native through six cold attachment waits`, async ({ page }) => {
  const fixture = await startDogfood(destination === 'new' ? 'mcp_startup' : 'mcp_startup_timeout');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser test retired')));
  let ownedGate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined;
  let passed = false;
  try {
    const gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    ownedGate = gate;
    if (destination === 'existing') appendFileSync(join(fixture.workspaceA, 'rustx.toml'), '\n[mcp_startup_policy]\nwait_timeout_ms = 1000\n');
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
    if (destination === 'new') {
      await chooseWorkspace(page, 'Workspace A');
      // Name the durable Session before its create acknowledgement continues;
      // this test owns only the primary provider request, not Title work.
      wire.holdNext('session/create');
    }
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('mcp startup under saturated attachments');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    let selected = { id: sessions[0].id, node: sessions[0].active_node };
    if (destination === 'new') {
      await expect.poll(() => wire.held()).toBe(1);
      const created = wire.responses.find(row => row.method === 'session/create')!.result.session;
      selected = { id: created.id, node: created.active_node };
      await activeNative.renameSession(created.id, 'New MCP under saturated attachments');
      expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(0);
      expect(wire.requests.filter(row => row.method === 'session/create')).toHaveLength(1);
      wire.releaseHeld();
    }
    await expect.poll(() => wire.requests.filter(row => row.method === 'session/submissionIntent').length).toBe(1);
    expect(wire.requests.find(row => row.method === 'session/submissionIntent')!.params).toEqual({ session_id: selected.id, node_id: selected.node });
    if (destination === 'new') {
      await expect.poll(() => gate.calls).toBe(7);
      expect((await fixture.control('requests')).requests).toEqual([]);
      // Release only the new Conversation's peer. All six original listing
      // gates stay closed throughout its complete first submission.
      gate.release();
    }
    // The existing target instead remains held until its configured deadline.
    const providerGate = destination === 'new' ? 'startup-request' : 'startup-timeout-request';
    await fixture.gate(providerGate);
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(1);
    if (destination === 'new') {
      expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
      expect(requests[0].body.tools[0].function.parameters).toEqual(startupToolSchema);
    } else expect(requests[0].body.tools ?? []).toEqual([]);
    expect(gate.calls).toBe(destination === 'new' ? 7 : 6);
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(1);
    const intent = wire.responses.findIndex(row => row.method === 'session/submissionIntent');
    const attached = wire.responses.findIndex(row => row.method === 'session/attach');
    expect(intent).toBeGreaterThanOrEqual(0);
    expect(intent).toBeLessThan(attached);
    await fixture.release(providerGate);
    await expect(page.getByText(destination === 'new' ? 'Ready tools received.' : 'Submission deadline applied.', { exact: true })).toBeVisible();
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

test('same-authority reconnect accepts first Send while its idle-evicted Session prepares MCP', async ({ page }) => {
  const fixture = await startDogfood('mcp_startup');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser test retired')));
  let ownedGate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined;
  let passed = false;
  try {
    const gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    ownedGate = gate;
    const activeNative = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    native = activeNative;
    // Use the existing User process policy and actual native residency signal;
    // no sleep or browser timer stands in for physical idle eviction.
    appendFileSync(fixture.settings, '\n[app_server]\nidle_grace_ms = 1\n');
    await activeNative.client.call('configuration/reconcile', { target: { kind: 'user' } }, 'configuration_receipt');
    await expect.poll(async () => (await activeNative.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.policy.idle_grace_ms).toBe(1);
    const created = await activeNative.createSession({ cwd: fixture.workspaceA });
    await activeNative.renameSession(created.session.id, 'Retained cold MCP');
    await page.addInitScript(({ endpoint, id }) => localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: [id] })), { endpoint: new URL(fixture.endpoint).href, id: created.session.id });
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await startupWait(() => gate.reached, cancellation.signal);
    expect(wire.requests.filter(row => row.method === 'session/attach')).toHaveLength(1);
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(0);
    gate.release();
    // Reconnect is permitted only after the first claim is acknowledged and
    // observed. Losing an unacknowledged Attach must remain uncertain.
    await expect.poll(() => wire.responses.filter(row => row.method === 'session/subscribe').length).toBe(1);
    const firstTarget = wire.responses.find(row => row.method === 'session/attach')!.result.target;
    await connectionAction(page, 'Disconnect');
    await expect.poll(async () => {
      const state = (await activeNative.client.call('server/diagnostics', {}, 'diagnostics')).snapshot;
      return { loaded: state.loaded, loading: state.loading, unloading: state.unloading, attachments: state.external_attachments };
    }).toEqual({ loaded: 0, loading: 0, unloading: 0, attachments: 0 });
    await connectionAction(page, 'Reconnect');
    await expect.poll(() => wire.requests.filter(row => row.method === 'session/attach').length).toBe(2);
    await expect.poll(() => gate.calls).toBe(2);
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(1);
    const identities = wire.responses.filter(row => row.method === 'initialize').map(row => row.result.authority_id);
    expect(identities).toHaveLength(2);
    expect(identities[1]).toBe(identities[0]);
    expect(wire.requests.filter(row => row.method === 'session/submissionIntent')).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toEqual([]);
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('mcp startup first Send after reconnect');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.responses.filter(row => row.method === 'session/submissionIntent').length).toBe(1);
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(1);
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(0);
    expect(gate.calls).toBe(2);
    expect((await fixture.control('requests')).requests).toEqual([]);
    gate.release();
    await fixture.gate('startup-request');
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(1);
    expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
    expect(requests[0].body.tools[0].function.parameters).toEqual(startupToolSchema);
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(1);
    const nextTarget = wire.responses.filter(row => row.method === 'session/attach')[1]!.result.target;
    expect(nextTarget.conversation_id).toBe(firstTarget.conversation_id);
    expect(nextTarget.runtime_incarnation).not.toBe(firstTarget.runtime_incarnation);
    // Attachment IDs are local to one runtime host and may repeat after
    // eviction; the complete target and incarnation establish authority.
    expect(nextTarget).not.toEqual(firstTarget);
    await fixture.release('startup-request');
    await expect(page.getByText('Ready tools received.', { exact: true })).toBeVisible();
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
