import { BROWSER_SESSION_HEADER, BROWSER_SESSION_PATTERN, BROWSER_SESSION_STORAGE } from '../../browser-session.ts';

/** Proof delivery is restricted to this page's exact origin and carrier APIs.
 * Never forward it through redirects or to an externally configured Host. */
export function carrierFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const url = new URL(path, location.href);
  if (url.origin !== location.origin || url.username || url.password
    || !(url.pathname === '/__rustx/bootstrap' || url.pathname.startsWith('/product-host/'))) throw new Error('Carrier APIs must be same-origin.');
  const headers = new Headers(init.headers);
  headers.delete(BROWSER_SESSION_HEADER);
  try {
    const proof = sessionStorage.getItem(BROWSER_SESSION_STORAGE);
    if (proof && BROWSER_SESSION_PATTERN.test(proof)) headers.set(BROWSER_SESSION_HEADER, proof);
  } catch { /* Storage unavailable: API authentication fails closed. */ }
  return fetch(url.href, { ...init, headers, credentials: 'omit', cache: 'no-store', redirect: 'error' });
}
