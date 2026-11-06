import { test, expect } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('Dogfood teardown drains an intercepted Host request before closing its owners', async ({ page }) => {
  const fixture = await startDogfood();
  let enter!: () => void, resume!: () => void;
  const entered = new Promise<void>(resolve => { enter = resolve; }), release = new Promise<void>(resolve => { resume = resolve; });
  let routeEntries = 0, responseStatus: number | undefined, stopped = false;
  let stopping: Promise<void> | undefined;
  try {
    await routeWorkspaceHost(page, fixture, {
      before: async () => { routeEntries++; enter(); await release; },
      observed: async () => {},
    }, (request, response) => {
      if (request.url().endsWith('/configuration')) responseStatus = response.status;
    });
    await page.goto('/');
    const request = { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ operation: { kind: 'write' } }) };
    // Invalid configuration is intentional: only the Host response, rather
    // than a native configuration mutation, is needed to exercise the proxy.
    await page.evaluate(request => { void fetch('/product-host/configuration', request); }, request);
    await entered;
    stopping = stopDogfoodPage(page, { stop: async check => {
      try {
        expect(page.isClosed()).toBe(true);
        expect(responseStatus).toBe(400);
      } finally {
        stopped = true;
        await fixture.stop(check);
      }
    } }, false);
    // unrouteAll removes producers synchronously before waiting for the held
    // callback. A later browser request must not enter that Node proxy again.
    await page.evaluate(request => fetch('/product-host/configuration', request).then(response => response.status), request);
    expect(routeEntries).toBe(1);
    expect(responseStatus).toBeUndefined();
    expect(page.isClosed()).toBe(false);
    expect(stopped).toBe(false);
    resume();
    await stopping;
    expect(stopped).toBe(true);
  } finally {
    resume();
    if (stopping) await stopping;
    else await stopDogfoodPage(page, fixture, false);
  }
});
