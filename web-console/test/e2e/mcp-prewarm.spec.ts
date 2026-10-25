import { expect, test, type Page } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { mcpStartupGate, startupToolSchema, startupWait } from '../../../tui/test/support/mcp-startup-gate';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openSettingsPage } from './shell-actions';
import { wireProbe } from './wire-probe';

async function chinese(page: Page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await openSettingsPage(page, 'General');
  await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
  await page.getByRole('button', { name: '关闭设置', exact: true }).click();
}
async function pick(page: Page, locale: string, name: string) {
  await page.getByRole('button', { name: locale === 'zh' ? '选择工作区' : 'Choose Workspace', exact: true }).click();
  await page.getByRole('menuitem', { name, exact: true }).click();
}
function absent(pid: number) {
  try { process.kill(pid, 0); return false; }
  catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return true; throw error; }
}

for (const width of [1440, 390]) for (const locale of ['en', 'zh']) test(`silent ready draft reuses one real process at ${width}px in ${locale}`, async ({ page }, info) => {
  const fixture = await startDogfood('mcp_startup');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser retired')));
  let gate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined;
  let passed = false;
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
  try {
    gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const wire = await wireProbe(page); await routeWorkspaceHost(page, fixture);
    await page.setViewportSize({ width, height: 900 }); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    if (locale === 'zh') await chinese(page);
    await expect(page).toHaveTitle(/rustX/);
    await expect(page.locator('html')).toHaveAttribute('lang', locale === 'zh' ? 'zh-CN' : 'en');
    expect((await native.listSessions()).sessions).toHaveLength(0);
    expect(gate.counts.processes).toBe(0);
    const selectedAt = performance.now();
    await pick(page, locale, 'Workspace A');
    await startupWait(() => gate!.reached, cancellation.signal);
    const editor = page.getByRole('textbox', { name: locale === 'zh' ? '消息' : 'Message', exact: true });
    const send = page.getByRole('button', { name: locale === 'zh' ? '发送' : 'Send', exact: true });
    await expect(editor).toBeEditable();
    await editor.fill('mcp startup from a ready draft'); await expect(send).toBeEnabled();
    const composer = page.locator('[data-resident-composer]');
    const pendingText = await composer.innerText();
    const pendingBounds = await composer.boundingBox();
    expect(wire.requests.filter(row => ['session/create', 'turn/start', 'session/uploadPrepare'].includes(row.method))).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    expect(gate.counts).toMatchObject({ processes: 1, initializations: 1, lists: 1 });
    await page.screenshot({ path: info.outputPath('silent-prewarm-pending.png'), fullPage: false });
    const releasedAt = performance.now();
    gate.release();
    await expect.poll(async () => (await native!.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.mcp_drafts.prepared).toBe(1);
    const readyAt = performance.now();
    expect(await composer.innerText()).toBe(pendingText);
    expect(await composer.boundingBox()).toEqual(pendingBounds);
    expect((await native.listSessions()).sessions).toHaveLength(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    await page.screenshot({ path: info.outputPath('silent-prewarm-ready.png'), fullPage: false });
    wire.holdNext('session/create');
    const sentAt = performance.now(); await send.click();
    await expect.poll(() => wire.held()).toBe(1);
    const created = wire.responses.find(row => row.method === 'session/create')!.result.session;
    await native.renameSession(created.id, 'Ready draft fixture'); wire.releaseHeld();
    await fixture.gate('startup-request');
    const requestedAt = performance.now();
    const requests = (await fixture.control('requests')).requests;
    expect(requests).toHaveLength(1);
    expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
    expect(requests[0].body.tools[0].function.parameters).toEqual(startupToolSchema);
    expect(gate.counts).toMatchObject({ processes: 1, initializations: 1, lists: 1, completedLists: 1 });
    expect(wire.requests.filter(row => row.method === 'session/create')).toHaveLength(1);
    expect(wire.requests.filter(row => row.method === 'turn/start')).toHaveLength(1);
    expect(wire.requests.filter(row => row.method === 'session/submissionIntent')).toHaveLength(0);
    await fixture.release('startup-request');
    await expect(page.getByText('Ready tools received.', { exact: true })).toBeVisible();
    expect(errors).toEqual([]);
    const evidence = { scenario: 'send_after_ready', width, locale, selection_to_ready_ms: readyAt - selectedAt, controlled_list_gate_ms: releasedAt - gate.events.find(event => event.kind === 'L')!.at, send_to_first_model_request_ms: requestedAt - sentAt, composer_bounds: pendingBounds, counts: gate.counts, processes: gate.processes, events: gate.events, first_request_tools: requests[0].body.tools, provider_requests: requests.length, clock: 'Node performance.now; readiness is the native diagnostic, request is the emulator gate', limits: 'Single fixture sample; includes the controlled discovery gate, browser, Host and observation overhead; no claim of faster MCP networking.' };
    writeFileSync(info.outputPath('prewarm-evidence.json'), JSON.stringify(evidence, null, 2) + '\n');
    await info.attach('prewarm-evidence', { path: info.outputPath('prewarm-evidence.json'), contentType: 'application/json' });
    passed = true;
  } finally {
    cancellation.abort(new Error('Test settled'));
    try { await gate?.close(); }
    finally { try { await page.close(); await native?.shutdown(); } finally { await fixture.stop(passed); } }
    for (const process of gate?.processes ?? []) { expect(absent(process.pid)).toBe(true); expect(absent(process.supervisorPid)).toBe(true); }
  }
});

for (const width of [1440, 390]) test(`A to B to A settles abandoned real processes and preserves ordered draft files at ${width}px`, async ({ page }, info) => {
  const fixture = await startDogfood('mcp_startup'); const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser retired')));
  let a: Awaited<ReturnType<typeof mcpStartupGate>> | undefined, b: typeof a;
  let native: AppServerHost | undefined; let passed = false;
  try {
    a = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    b = await mcpStartupGate(fixture.workspaceB, cancellation.signal, 'abandoned_b_echo');
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const wire = await wireProbe(page); await routeWorkspaceHost(page, fixture);
    await page.setViewportSize({ width, height: 900 }); await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await pick(page, 'en', 'Workspace A'); await startupWait(() => a!.reached, cancellation.signal);
    const editor = page.getByRole('textbox', { name: 'Message', exact: true }); await editor.fill('mcp startup with preserved draft');
    await page.getByLabel('Attach files').setInputFiles([{ name: 'first.txt', mimeType: 'text/plain', buffer: Buffer.from('first') }, { name: 'second.txt', mimeType: 'text/plain', buffer: Buffer.from('second') }]);
    const switchAt = performance.now();
    await pick(page, 'en', 'Workspace B'); await startupWait(() => b!.reached, cancellation.signal);
    expect(absent(a.processes[0].pid)).toBe(true); expect(absent(a.processes[0].supervisorPid)).toBe(true);
    const aSettledAt = performance.now();
    await pick(page, 'en', 'Workspace A'); await expect.poll(() => a!.calls).toBe(2);
    expect(absent(b.processes[0].pid)).toBe(true); expect(absent(b.processes[0].supervisorPid)).toBe(true);
    await expect(editor).toHaveValue('mcp startup with preserved draft');
    await expect(page.getByText('first.txt', { exact: true })).toBeVisible(); await expect(page.getByText('second.txt', { exact: true })).toBeVisible();
    expect(wire.requests.filter(row => ['session/create', 'session/uploadPrepare', 'turn/start'].includes(row.method))).toHaveLength(0);
    wire.holdNext('session/create'); const sentAt = performance.now(); await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.held()).toBe(1);
    const created = wire.responses.find(row => row.method === 'session/create')!.result.session;
    await native.renameSession(created.id, 'Switched draft fixture'); wire.releaseHeld();
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    a.release(); await fixture.gate('startup-request'); const requestedAt = performance.now();
    const requests = (await fixture.control('requests')).requests; expect(requests).toHaveLength(1);
    expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
    expect(wire.requests.filter(row => row.method === 'session/uploadPrepare').flatMap(row => row.params.files.map((file: { name: string }) => file.name))).toEqual(['first.txt', 'second.txt']);
    expect(a.counts).toMatchObject({ processes: 2, initializations: 2, lists: 2 }); expect(b.counts).toMatchObject({ processes: 1, initializations: 1, lists: 1 });
    writeFileSync(info.outputPath('prewarm-switch-evidence.json'), JSON.stringify({ scenario: 'switch_pending_then_send', width, a: a.counts, b: b.counts, first_request_tools: requests[0].body.tools, provider_requests: requests.length, switch_to_old_process_settled_ms: aSettledAt - switchAt, cancellation_to_settlement_upper_bound_ms: aSettledAt - wire.requestTimes.get(wire.requests.find(row => row.method === 'mcp/draftCancel')!.id)!, send_to_first_model_request_ms: requestedAt - sentAt, processes: [...a.processes, ...b.processes] }, null, 2) + '\n');
    await info.attach('prewarm-switch-evidence', { path: info.outputPath('prewarm-switch-evidence.json'), contentType: 'application/json' });
    await fixture.release('startup-request'); await expect(page.getByText('Ready tools received.', { exact: true })).toBeVisible(); passed = true;
  } finally {
    cancellation.abort(new Error('Test settled'));
    try { await a?.close(); await b?.close(); }
    finally { try { await page.close(); await native?.shutdown(); } finally { await fixture.stop(passed); } }
    for (const process of [...a?.processes ?? [], ...b?.processes ?? []]) { expect(absent(process.pid)).toBe(true); expect(absent(process.supervisorPid)).toBe(true); }
  }
});


test('immediate Send measures the pending discovery on one real process', async ({ page }, info) => {
  const fixture = await startDogfood('mcp_startup');
  const cancellation = new AbortController();
  page.once('close', () => cancellation.abort(new Error('Browser retired')));
  let gate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let native: AppServerHost | undefined; let passed = false;
  try {
    gate = await mcpStartupGate(fixture.workspaceA, cancellation.signal);
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const wire = await wireProbe(page); await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('mcp startup sent immediately');
    wire.holdNext('session/create');
    const selectedAt = performance.now(); await pick(page, 'en', 'Workspace A');
    const sentAt = performance.now(); await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.held()).toBe(1);
    const created = wire.responses.find(row => row.method === 'session/create')!.result.session;
    await native.renameSession(created.id, 'Immediate Send fixture'); wire.releaseHeld();
    await startupWait(() => gate!.reached, cancellation.signal);
    expect(gate.counts).toMatchObject({ processes: 1, initializations: 1, lists: 1, completedLists: 0 });
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    const releaseAt = performance.now(); gate.release();
    await fixture.gate('startup-request'); const requestedAt = performance.now();
    const requests = (await fixture.control('requests')).requests; expect(requests).toHaveLength(1);
    expect(requests[0].body.tools.map((tool: { function: { name: string } }) => tool.function.name)).toEqual(['startup_echo']);
    expect(requests[0].body.tools[0].function.parameters).toEqual(startupToolSchema);
    await expect.poll(() => gate!.counts.completedLists).toBe(1);
    expect(gate.counts).toMatchObject({ processes: 1, initializations: 1, lists: 1, completedLists: 1 });
    // T observes the peer's response-flush receipt. The model request bounds
    // native readiness above; the independent receipt is no exact native clock.
    const flushedAt = gate.events.find(event => event.kind === 'T')!.at;
    writeFileSync(info.outputPath('prewarm-immediate-evidence.json'), JSON.stringify({ scenario: 'immediate_send',
      selection_to_list_response_flush_observation_ms: flushedAt - selectedAt, selection_to_native_readiness_upper_bound_ms: requestedAt - selectedAt,
      send_to_first_model_request_ms: requestedAt - sentAt, deliberate_gate_hold_after_send_ms: releaseAt - sentAt,
      counts: gate.counts, processes: gate.processes, events: gate.events, first_request_tools: requests[0].body.tools, provider_requests: requests.length,
      limits: 'Single controlled fixture; includes browser, Host, durable ACK and observer overhead. Flush receipt and model request are independent observer events; only the model request bounds native readiness above.' }, null, 2) + '\n');
    await info.attach('prewarm-immediate-evidence', { path: info.outputPath('prewarm-immediate-evidence.json'), contentType: 'application/json' });
    await fixture.release('startup-request'); await expect(page.getByText('Ready tools received.', { exact: true })).toBeVisible(); passed = true;
  } finally {
    cancellation.abort(new Error('Test settled'));
    try { await gate?.close(); }
    finally { try { await page.close(); await native?.shutdown(); } finally { await fixture.stop(passed); } }
    for (const process of gate?.processes ?? []) { expect(absent(process.pid)).toBe(true); expect(absent(process.supervisorPid)).toBe(true); }
  }
});
