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
    // Retirement fences new Host work while the admitted callback drains.
    expect(await page.evaluate(request => fetch('/product-host/configuration', request).then(response => response.status), request)).toBe(503);
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

test('Dogfood teardown drains staggered Host callbacks before removing interception', async ({ page }) => {
  const fixture = await startDogfood();
  const resumes: (() => void)[] = [];
  let enter!: () => void;
  const entered = new Promise<void>(resolve => { enter = resolve; });
  let stopping: Promise<void> | undefined;
  try {
    await routeWorkspaceHost(page, fixture, {
      before: () => new Promise<void>(resolve => { resumes.push(resolve); if (resumes.length === 2) enter(); }),
      observed: async () => {},
    });
    await page.goto('/');
    await page.evaluate(() => {
      Object.assign(window, { proxyResponses: 0 });
      for (let index = 0; index < 2; index++) void fetch('/product-host/configuration', {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ operation: { kind: 'write' } }),
      }).then(() => { (window as unknown as { proxyResponses: number }).proxyResponses++; });
    });
    await entered;
    stopping = stopDogfoodPage(page, fixture, false);
    resumes[0]();
    await expect.poll(() => page.evaluate(() => (window as unknown as { proxyResponses: number }).proxyResponses)).toBe(1);
    expect(page.isClosed()).toBe(false);
    resumes[1]();
    await stopping;
  } finally {
    resumes.forEach(resume => resume());
    if (stopping) await stopping;
    else await stopDogfoodPage(page, fixture, false);
  }
});
