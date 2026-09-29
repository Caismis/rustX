import { afterEach, expect, it, vi } from 'vitest';
import { carrierFetch } from '../src/carrier/http';
import { WorkspaceHostError } from '../src/workspaces/host';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { BROWSER_SESSION_HEADER, BROWSER_SESSION_STORAGE } from '../browser-session';
afterEach(() => { sessionStorage.clear(); vi.unstubAllGlobals(); });
it('proof goes only to same-origin bootstrap/Host calls, never redirects or external bases', async () => {
  const proof = 'P'.repeat(43); sessionStorage.setItem(BROWSER_SESSION_STORAGE, proof);
  const fetcher = vi.fn(async () => new Response('{}')); vi.stubGlobal('fetch', fetcher);
  await carrierFetch('/__rustx/bootstrap'); await new HttpWorkspaceHost().listWorkspaces();
  expect((fetcher.mock.calls as unknown as [string, RequestInit][]).map(([url]) => new URL(url).origin)).toEqual([location.origin, location.origin]);
  for (const [, init] of fetcher.mock.calls as unknown as [string, RequestInit][]) {
    expect(new Headers(init.headers).get(BROWSER_SESSION_HEADER)).toBe(proof);
    expect(init).toMatchObject({ credentials: 'omit', redirect: 'error', cache: 'no-store' });
  }
  expect(() => carrierFetch('http://127.0.0.1:9/__rustx/bootstrap')).toThrow('same-origin');
  await expect(new HttpWorkspaceHost('http://127.0.0.1:9/product-host').listWorkspaces()).rejects.toThrow('same-origin');
  expect(() => carrierFetch('/public')).toThrow('same-origin'); expect(fetcher).toHaveBeenCalledTimes(2);
});

it('browser decoding retains shared Host error identity, native kinds and uncertain outcomes', async () => {
  const host = new HttpWorkspaceHost();
  for (const [failure, kind, uncertain] of [
    [{ message: 'Workspace Host authority replaced', kind: 'authority_replaced' }, 'authority_replaced', false],
    [{ nativeError: { message: 'Revision changed', data: { kind: 'source_conflict' } } }, 'source_conflict', false],
    [{ message: 'Connection lost', uncertain: true }, undefined, true],
  ] as const) {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(failure), { status: 400 })));
    await expect(host.classifyLocations(['/workspace'], 'ws://localhost/', 'old-host')).rejects.toSatisfy((error: unknown) =>
      error instanceof WorkspaceHostError && error.kind === kind && error.uncertain === uncertain);
  }
});

it('browser classification validates results and forwards cancellation', async () => {
  const controller = new AbortController();
  const fetcher = vi.fn(async () => new Response(JSON.stringify([{ authorized: false, reason: 'unavailable' }])));
  vi.stubGlobal('fetch', fetcher);
  await expect(new HttpWorkspaceHost().classifyLocations(['/workspace'], 'ws://localhost/', 'host', controller.signal)).resolves.toEqual([{ authorized: false, reason: 'unavailable' }]);
  expect((fetcher.mock.calls as unknown as [string, RequestInit][])[0][1].signal).toBe(controller.signal);
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify([{ authorized: false }]))));
  await expect(new HttpWorkspaceHost().classifyLocations(['/workspace'], 'ws://localhost/')).rejects.toThrow('Invalid Workspace classification');
});
