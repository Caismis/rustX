import { afterEach, expect, it, vi } from 'vitest';
import { carrierFetch } from '../src/carrier/http';
import { HttpWorkspaceHost } from '../src/workspaces/host';
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
