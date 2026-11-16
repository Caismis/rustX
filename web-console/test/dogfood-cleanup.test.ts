// @vitest-environment node
import { createServer } from 'node:http';
import type { Page, Route } from '@playwright/test';
import { expect, it, vi } from 'vitest';
import { stopDogfoodPage } from './e2e/dogfood-server';
import { routeWorkspaceHost } from './e2e/workspace-host';

it('drains a held localhost proxy response before retiring the page and fixture, returning its exact report', async () => {
  let release!: () => void, received!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; }), requested = new Promise<void>(resolve => { received = resolve; });
  const events: string[] = [];
  const server = createServer((_request, response) => { received(); void held.then(() => { response.end('settled'); }); });
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('Missing fixture port');
  let forward!: (route: Route) => Promise<void>;
  const page = {
    route: vi.fn(async (_pattern: string, handler: (route: Route) => Promise<void>) => { forward = handler; }),
    unrouteAll: vi.fn(async () => { events.push('unroute'); }),
    close: vi.fn(async () => { events.push('page'); }),
  } as unknown as Page;
  const report = { ok: true, requestCount: 0 };
  const fixture = {
    workspaceHostUrl: `http://127.0.0.1:${address.port}`,
    stop: vi.fn(async () => { events.push('fixture'); await new Promise<void>((resolve, reject) => { server.close(error => error ? reject(error) : resolve()); }); return report; }),
  };
  let forwarding: Promise<void> | undefined, cleanup: Promise<unknown> | undefined;
  try {
    await routeWorkspaceHost(page, fixture);
    const route = {
      request: () => ({ url: () => 'http://app/product-host/configuration', postDataJSON: () => ({ operation: { kind: 'read' } }), postData: () => '{"operation":{"kind":"read"}}' }),
      fulfill: vi.fn(async (response: { status: number; body: Buffer }) => { expect(response.status).toBe(200); expect(response.body.toString()).toBe('settled'); events.push('response'); }),
    } as unknown as Route;
    forwarding = forward(route); await requested;
    cleanup = stopDogfoodPage(page, fixture, false);
    expect(server.listening).toBe(true); expect(fixture.stop).not.toHaveBeenCalled(); expect(page.unrouteAll).not.toHaveBeenCalled();
    release(); await forwarding;
    expect(await cleanup).toBe(report);
    expect(events).toEqual(['response', 'unroute', 'page', 'fixture']);
    expect(page.unrouteAll).toHaveBeenCalledExactlyOnceWith({ behavior: 'wait' }); expect(fixture.stop).toHaveBeenCalledExactlyOnceWith(false);
  } finally {
    release(); await Promise.allSettled([forwarding, cleanup]);
    server.closeAllConnections(); if (server.listening) await new Promise<void>(resolve => { server.close(() => resolve()); });
  }
});

it.each(['unrouteAll', 'close'] as const)('preserves a %s failure while still stopping the fixture', async failed => {
  const error = new Error(`Failed ${failed}`), report = { ok: true, requestCount: 0 };
  const page = { unrouteAll: vi.fn(async () => {}), close: vi.fn(async () => {}) };
  page[failed].mockRejectedValueOnce(error);
  const fixture = { stop: vi.fn(async () => report) };
  await expect(stopDogfoodPage(page as unknown as Page, fixture)).rejects.toBe(error);
  expect(page.close).toHaveBeenCalledOnce(); expect(fixture.stop).toHaveBeenCalledExactlyOnceWith(true);
});
