import { test, expect, type Page, type TestInfo } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, closeSessionView } from './shell-actions';

type RequestTiming = { method: string; start: number; end?: number; bytes?: number; error?: string };
type HttpTiming = { path: string; start: number; end?: number };
type Mark = { kind: 'click' | 'selected' | 'phase'; name: string; time: number };
interface SessionLatency { marks: Mark[]; rpc: RequestTiming[]; http: HttpTiming[] }
declare global { interface Window { __sessionLatency: SessionLatency } }

/** Observe the production client using one browser performance clock. No RPC
 * interception, extra native reads, artificial sleeps or mutation replay. Only
 * method/path/timing metadata is recorded; request and response bodies stay private. */
async function observeLatency(page: Page) {
  await page.addInitScript(() => {
    const evidence: SessionLatency = { marks: [], rpc: [], http: [] };
    window.__sessionLatency = evidence;
    const NativeSocket = window.WebSocket;
    window.WebSocket = class extends NativeSocket {
      private calls = new Map<unknown, RequestTiming>();
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        this.addEventListener('message', event => {
          const reply = JSON.parse(event.data) as { id?: unknown; error?: { data?: { kind?: string } } };
          const row = this.calls.get(reply.id);
          if (row) { row.end = performance.now(); row.bytes = event.data.length; row.error = reply.error?.data?.kind; this.calls.delete(reply.id); }
        });
      }
      send(data: Parameters<WebSocket['send']>[0]) {
        if (typeof data === 'string') {
          const call = JSON.parse(data) as { id?: unknown; method?: string };
          if (call.id !== undefined && call.method) {
            const row: RequestTiming = { method: call.method, start: performance.now() };
            evidence.rpc.push(row); this.calls.set(call.id, row);
          }
        }
        super.send(data);
      }
    };
    const originalFetch = window.fetch;
    window.fetch = async (...args) => {
      const resource = args[0], path = new URL(resource instanceof Request ? resource.url : String(resource), location.href).pathname;
      if (!path.startsWith('/product-host/')) return originalFetch(...args);
      const row: HttpTiming = { path, start: performance.now() }; evidence.http.push(row);
      try { return await originalFetch(...args); } finally { row.end = performance.now(); }
    };
    document.addEventListener('click', event => {
      const button = event.target instanceof Element ? event.target.closest('button') : null;
      if (button) evidence.marks.push({ kind: 'click', name: button.getAttribute('aria-label') ?? button.textContent ?? '', time: performance.now() });
    }, true);
    let selected: string | null = null;
    new MutationObserver(() => {
      const id = document.querySelector('button[data-session-id][aria-current="page"]')?.getAttribute('data-session-id') ?? null;
      if (id !== selected) { selected = id; evidence.marks.push({ kind: 'selected', name: id ?? '', time: performance.now() }); }
    }).observe(document, { childList: true, subtree: true, attributes: true });
  });
}
const mark = (page: Page, name: string) => page.evaluate(name => window.__sessionLatency.marks.push({ kind: 'phase', name, time: performance.now() }), name);
async function report(page: Page, info: TestInfo) {
  const evidence = await page.evaluate(() => window.__sessionLatency);
  await info.attach('session-latency.json', { body: JSON.stringify(evidence, null, 2), contentType: 'application/json' });
  console.log('SESSION_LATENCY', JSON.stringify(evidence));
  expect(evidence.rpc.filter(row => row.error)).toEqual([]);
  return evidence;
}

// These are diagnostic measurements, not machine-speed assertions. Native
// operation counts and ordering remain deterministic; wall times are evidence.
test('measure native Fork, warm navigation and resident reattachment', async ({ page }, info) => {
  const fixture = await startDogfood('web_fork_feedback');
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture); await observeLatency(page);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Fork source');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Completed fork answer.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Regenerate', exact: true })).toBeEnabled();
    const source = (await page.locator('button[data-session-id][aria-current="page"]').getAttribute('data-session-id'))!;
    await mark(page, 'fork');
    await page.getByRole('button', { name: 'Branch into a new Session', exact: true }).click();
    await expect(page.locator('button[data-session-id][aria-current="page"]')).not.toHaveAttribute('data-session-id', source);
    await expect(page.getByRole('button', { name: 'Regenerate', exact: true })).toBeEnabled();
    const child = (await page.locator('button[data-session-id][aria-current="page"]').getAttribute('data-session-id'))!;
    await mark(page, 'warm'); await page.locator(`button[data-session-id="${source}"]`).click();
    await expect(page.locator(`button[data-session-id="${source}"]`)).toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('button', { name: 'Regenerate', exact: true })).toBeEnabled();
    await mark(page, 'close-view'); await closeSessionView(page, child);
    // Detaching this browser view does not unload the native runtime. Do not
    // describe this as cold composition or compare it to first-open MCP startup.
    await mark(page, 'reattach-resident'); await page.locator(`button[data-session-id="${child}"]`).click();
    await expect(page.locator(`button[data-session-id="${child}"]`)).toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('button', { name: 'Regenerate', exact: true })).toBeEnabled();
    const evidence = await report(page, info);
    expect(evidence.rpc.filter(row => row.method === 'session/fork')).toHaveLength(1);
    expect(evidence.rpc.filter(row => row.method === 'turn/start')).toHaveLength(1);
    const start = evidence.marks.find(row => row.kind === 'phase' && row.name === 'warm')!.time;
    const end = evidence.marks.find(row => row.kind === 'phase' && row.name === 'close-view')!.time;
    expect(evidence.rpc.filter(row => row.method === 'session/attach' && row.start > start && row.start < end)).toHaveLength(0);
    passed = true;
  } finally { await page.close(); await fixture.stop(passed); }
});

test('measure Retry preparation separately from model generation', async ({ page }, info) => {
  const fixture = await startDogfood('web_response_retry');
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture); await observeLatency(page);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    for (const [input, output] of [['Earlier question', 'Earlier answer stays.'], ['Retry this question', 'Old reply to replace.']]) {
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill(input);
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(page.getByText(output, { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Regenerate', exact: true }).last()).toBeEnabled();
    }
    await mark(page, 'retry'); await page.getByRole('button', { name: 'Regenerate', exact: true }).last().click();
    // The actual provider's gate proves the replacement request reached it.
    // Its held generation time is excluded from the preparation measurement.
    await fixture.gate('replay-request'); await mark(page, 'provider-request-received');
    await fixture.release('replay-request'); await fixture.gate('replay-streaming');
    await expect(page.getByText('Generating the replacement.', { exact: true })).toBeVisible();
    await fixture.release('replay-streaming'); await expect(page.getByText('Replacement complete.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Regenerate', exact: true }).last()).toBeEnabled();
    const evidence = await report(page, info);
    const branch = evidence.rpc.filter(row => row.method === 'session/branch');
    const switching = evidence.rpc.filter(row => row.method === 'session/switchNode');
    const attach = evidence.rpc.filter(row => row.method === 'session/attach').at(-1)!;
    const input = evidence.rpc.filter(row => row.method === 'turn/start');
    expect(branch).toHaveLength(1); expect(switching).toHaveLength(1); expect(input).toHaveLength(3);
    expect(switching[0].start).toBeGreaterThanOrEqual(branch[0].end!);
    expect(attach.start).toBeGreaterThanOrEqual(switching[0].end!);
    expect(input[2].start).toBeGreaterThanOrEqual(attach.end!);
    passed = true;
  } finally { await page.close(); await fixture.stop(passed); }
});
