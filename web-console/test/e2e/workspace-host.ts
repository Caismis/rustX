import { createServer } from 'node:http';
import type { Page } from '@playwright/test';
import { LocalWorkspaceHost, type LocalHostConfig } from '../../host/workspaces.ts';
import { workspaceHandler } from '../../host/http.ts';
export async function startWorkspaceHost(config: LocalHostConfig) {
  const host = new LocalWorkspaceHost(config);
  const server = createServer(workspaceHandler(host));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Host address missing');
  return { host, url: `http://127.0.0.1:${address.port}`, stop: () => !server.listening ? Promise.resolve() : new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve())) };
}
/** Same-origin deployment proxy to the actual isolated Node Product Host fixture. */
export async function routeWorkspaceHost(page: Page, fixture: { workspaceHostUrl: string }, writeGate?: { before: () => Promise<void>; observed: () => Promise<void> }) {
  await page.route('**/product-host/*', async route => {
    const write = route.request().url().endsWith('/configuration') && route.request().postDataJSON()?.operation?.kind === 'write';
    if (write) await writeGate?.before();
    const response = await fetch(`${fixture.workspaceHostUrl}${new URL(route.request().url()).pathname}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: route.request().postData(),
    });
    if (write) await writeGate?.observed();
    await route.fulfill({ status: response.status, contentType: response.headers.get('content-type') ?? 'application/json', body: await response.text() });
  });
}
