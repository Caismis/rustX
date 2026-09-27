import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, chooseWorkspace } from './shell-actions';

// Identical production build + isolated native/provider fixture before and after.
// All times below come from one browser performance clock. No cross-process subtraction.
for (let sample = 1; sample <= 3; sample++) test(`startup phase measurement ${sample}`, async ({ page }) => {
  const fixture = await startDogfood('web_composer_context');
  try {
    await routeWorkspaceHost(page, fixture);
    await page.addInitScript(() => {
      const evidence = { identities: {} as Record<string, unknown>, rpcIds: [] as { id: unknown; method: string }[], times: {} as Record<string, number>, requests: [] as string[], startupRequests: [] as string[], providerTimings: [] as unknown[] };
      (window as any).__startup = evidence;
      const mark = (key: string) => { evidence.times[key] ??= performance.now(); };
      const nativeFetch = window.fetch;
      window.fetch = async (...args) => { const response = await nativeFetch(...args); if (evidence.times.T0 !== undefined && String(args[0]).includes('resolve')) mark('T1'); return response; };
      const NativeSocket = window.WebSocket;
      window.WebSocket = class extends NativeSocket {
        private methods = new Map<string, string>();
        constructor(url: string | URL, protocols?: string | string[]) {
          super(url, protocols);
          this.addEventListener('message', event => {
            const response = JSON.parse(event.data), method = this.methods.get(response.id);
            if (method === 'session/create') { mark('T3'); evidence.identities.created = response.result?.session; }
            if (method === 'session/attach') { evidence.identities.attached = response.result?.target; mark('T6'); if (response.result?.snapshot?.model) mark('T7'); }
            if (response.method === 'session/event' && ['assistant_text_delta', 'assistant_reasoning_delta', 'assistant_refusal_delta', 'tool_call_arguments_delta'].includes(response.params.event.type)) mark('T10');
            const trace = response.result?.snapshot?.trace?.records;
            if (trace) evidence.providerTimings = trace.filter((r: any) => r.request?.generation).map((r: any) => ({ request_id: r.request.request_id, generation: r.request.generation }));
            if (method === 'turn/start') mark('T9');
          });
        }
        send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
          if (typeof data === 'string') {
            const request = JSON.parse(data);
            this.methods.set(request.id, request.method);
            if (evidence.times.T0 !== undefined) { evidence.requests.push(request.method); evidence.rpcIds.push({ id: request.id, method: request.method }); }
            if (request.method === 'session/create') mark('T2');
            if (request.method === 'session/attach') mark('T4');
            if (request.method === 'turn/start') { mark('T8'); evidence.startupRequests = [...evidence.requests]; }
          }
          super.send(data);
        }
      };
      document.addEventListener('click', event => {
        if ((event.target as Element).closest('button')?.getAttribute('aria-label') === 'Send') mark('T0');
      }, true);
      new MutationObserver(() => {
        if (evidence.times.T3 && document.querySelector('#session-view[data-phase="active"]')) mark('visible');
        if (document.body.textContent?.includes('Plan recorded.')) mark('outputVisible');
      }).observe(document, { childList: true, subtree: true, attributes: true });
    });
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Plan the composer docks');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Plan recorded.', { exact: true })).toBeVisible();
    const result = await page.evaluate(() => (window as any).__startup);
    console.log('STARTUP_MEASUREMENT', JSON.stringify({ sample, ...result }));
    expect(result.times.T9).toBeGreaterThan(result.times.T3);
  } finally { await fixture.stop(false); }
});
