import { createServer } from 'node:http';
import type { Page, Request } from '@playwright/test';
import { LocalWorkspaceHost, type LocalHostConfig } from '../../host/workspaces.ts';
import { workspaceHandler } from '../../host/http.ts';
export async function startWorkspaceHost(config: LocalHostConfig) {
  const host = new LocalWorkspaceHost(config);
  const handler = workspaceHandler(host);
  const server = createServer((request, response) => {
    // This fixture is the reverse-proxy hop for the streaming route below.
    // Chromium preserves its original Origin when Playwright rewrites the URL;
    // mirror the buffered Node proxy's Origin removal without changing the
    // production Host's same-origin check.
    if (request.url?.endsWith('/file-observe-events')) {
      const origin = request.headers.origin;
      const allowed = [process.env.RUSTX_E2E_PREVIEW_PORT ?? '5173', process.env.RUSTX_E2E_FIXTURE_PORT ?? '5174'].some(port => origin === `http://127.0.0.1:${port}`);
      if (allowed) { response.setHeader('Access-Control-Allow-Origin', origin!); response.setHeader('Access-Control-Expose-Headers', 'X-Rustx-Observation-Channel'); delete request.headers.origin; }
    }
    void handler(request, response);
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Host address missing');
  return { host, url: `http://127.0.0.1:${address.port}`, stop: () => { host.close(); return !server.listening ? Promise.resolve() : new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve())); } };
}
export type WorkspaceHostResponse = { status: number; headers: Record<string, string>; body: Buffer };
/** Same-origin deployment proxy to the actual isolated Node Product Host fixture. */
export async function routeWorkspaceHost(page: Page, fixture: { workspaceHostUrl: string }, writeGate?: { before: () => Promise<void>; observed: () => Promise<void> }, observeResponse?: (request: Request, response: WorkspaceHostResponse) => void) {
  await page.route('**/product-host/*', async route => {
    // Observation is an open NDJSON response. Preserve its streaming transport;
    // buffering route.fulfill would withhold readiness until the watch closes.
    if (route.request().url().endsWith('/file-observe-events')) {
      await route.continue({ url: `${fixture.workspaceHostUrl}/product-host/file-observe-events` }); return;
    }
    const write = route.request().url().endsWith('/configuration') && route.request().postDataJSON()?.operation?.kind === 'write';
    if (write) await writeGate?.before();
    const response = await fetch(`${fixture.workspaceHostUrl}${new URL(route.request().url()).pathname}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: route.request().postData(),
    });
    if (write) await writeGate?.observed();
    const buffered = { status: response.status, headers: Object.fromEntries(response.headers), body: Buffer.from(await response.arrayBuffer()) };
    // Observe the same terminal bytes delivered below, without retrieving a
    // synthetic response body through Chromium's resource cache.
    observeResponse?.(route.request(), buffered);
    await route.fulfill(buffered);
  });
}
