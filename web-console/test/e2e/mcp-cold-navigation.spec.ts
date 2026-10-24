import { test, expect } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { mcpStartupGate, startupToolSchema, startupWait } from '../../../tui/test/support/mcp-startup-gate';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const width of [1440, 390]) for (const origin of ['session', 'workflows'] as const) test(`selecting cold MCP B from ${origin} at ${width}px admits only B's explicit first Send before Attach`, async ({ page }) => {
  const fixture = await startDogfood('mcp_startup');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser test retired')));
  let gate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined;
  let passed = false;
  try {
    gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const source = (await native.createSession({ cwd: fixture.workspaceB })).session;
    const target = (await native.createSession({ cwd: fixture.workspaceA })).session;
    await native.renameSession(source.id, 'Warm source A');
    await native.renameSession(target.id, 'Cold target B');
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    await page.locator(`button[data-session-id="${source.id}"]`).click();
    await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
    const sourceTarget = await wire.attachedTarget(source.id);
    const sourceAttempt = (await wire.read('session/checkpoint', sourceTarget)).checkpoint.snapshot.attempt;
    const editor = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(editor).toBeEnabled();
    await editor.fill('This unsent draft belongs to A');
    await page.getByLabel('Attach files').setInputFiles({ name: 'A-owned.txt', mimeType: 'text/plain', buffer: Buffer.from('A ownership') });
    await expect(page.locator('[data-attachment-state=ready]')).toBeVisible();
    const sourceUploads = wire.requests.filter(row => row.method === 'session/uploadPrepare');
    expect(sourceUploads).toHaveLength(1);
    expect(sourceUploads[0].params.target).toEqual(sourceTarget);
    if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    if (origin === 'workflows') {
      await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
      await expect(page.getByRole('heading', { name: 'Workflows', exact: true })).toBeVisible();
    }
    if (width === 390 && origin === 'workflows') await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    await page.locator(`button[data-session-id="${target.id}"]`).click();
    await startupWait(() => gate!.reached, cancellation.signal);
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session-draft');
    if (width === 390) await page.getByRole('button', { name: 'Close navigation', exact: true }).click({ position: { x: 370, y: 400 } });
    await expect(page.locator('main')).not.toHaveAttribute('inert');
    await expect(editor).toBeEnabled(); await expect(editor).toHaveValue('');
    await expect(page.getByText('A-owned.txt', { exact: true })).toHaveCount(0);
    expect(wire.requests.filter(row => row.method === 'session/submissionIntent')).toHaveLength(0);
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    expect((await wire.read('session/checkpoint', sourceTarget)).checkpoint.snapshot.attempt).toEqual(sourceAttempt);
    expect(wire.requests.filter(row => ['session/detach', 'turn/cancel', 'session/switchNode'].includes(row.method))).toEqual([]);

    await editor.fill('mcp startup input belongs only to B');
    await page.getByLabel('Attach files').setInputFiles({ name: 'B-owned.txt', mimeType: 'text/plain', buffer: Buffer.from('B ownership') });
    expect(wire.requests.filter(row => row.method === 'session/uploadPrepare')).toHaveLength(1);
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.responses.filter(row => row.method === 'session/submissionIntent').length).toBe(1);
    expect(wire.requests.find(row => row.method === 'session/submissionIntent')!.params).toEqual({ session_id: target.id, node_id: target.active_node });
    expect(wire.responses.filter(row => row.method === 'session/attach')).toHaveLength(1); // A only.
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(0);
    expect(wire.requests.filter(row => row.method === 'session/uploadPrepare')).toHaveLength(1);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    gate.release();
    await fixture.gate('startup-request');
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(1);
    expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
    expect(requests[0].body.tools[0].function.parameters).toEqual(startupToolSchema);
    expect(JSON.stringify(requests[0].body)).not.toContain('This unsent draft belongs to A');
    expect(JSON.stringify(requests[0].body)).not.toContain('A-owned.txt');
    const sent = wire.requests.filter(row => row.method === 'turn/start');
    expect(sent).toHaveLength(1); expect(sent[0].params.target.session_id).toBe(target.id);
    const uploads = wire.requests.filter(row => row.method === 'session/uploadPrepare');
    expect(uploads).toHaveLength(2);
    expect(uploads[1].params).toMatchObject({ target: { session_id: target.id }, files: [{ name: 'B-owned.txt' }] });
    expect((await wire.read('session/checkpoint', sourceTarget)).checkpoint.snapshot.attempt).toEqual(sourceAttempt);
    expect(gate.calls).toBe(1);
    await fixture.release('startup-request');
    await expect(page.getByText('Ready tools received.', { exact: true })).toBeVisible();
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await expect(editor).toBeEditable();
    await editor.fill('B remains usable after its first native acceptance');
    expect((await fixture.control('requests')).requests).toHaveLength(1);
    expect(wire.requests.filter(row => row.method === 'session/submissionIntent')).toHaveLength(1);
    passed = true;
  } finally {
    cancellation.abort(new Error('Browser test settled'));
    try { await gate?.close(); }
    finally {
      try { await page.close(); await native?.shutdown(); }
      finally { await fixture.stop(passed); }
    }
  }
});
