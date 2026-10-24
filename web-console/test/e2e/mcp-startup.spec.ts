import { expect, test } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { mcpStartupGate, startupToolSchema } from '../../../tui/test/support/mcp-startup-gate';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { chooseWorkspace, connectRemote } from './shell-actions';
import { wireProbe } from './wire-probe';

test('Web first Send waits for live MCP discovery and the first provider request has the exact tools', async ({ page }) => {
  const fixture = await startDogfood('mcp_startup');
  const gate = await mcpStartupGate(fixture.workspaceA);
  const wire = await wireProbe(page);
  wire.holdNext('session/create');
  let passed = false;
  try {
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
    await gate.reached;
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
  } finally {
    await fixture.stop(passed);
    await gate.close();
  }
});
