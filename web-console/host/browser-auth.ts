import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { BROWSER_SESSION_HEADER, BROWSER_SESSION_STORAGE, BROWSER_SESSION_PATTERN } from '../browser-session.ts';

export interface WebBootstrapConfig { appServerEndpoint: string; transportTokenFile: string; browserLaunchToken: string }
const secretPattern = BROWSER_SESSION_PATTERN;
function matches(actual: string | undefined, expected: string) {
  return actual !== undefined && secretPattern.test(actual) && timingSafeEqual(Buffer.from(actual), Buffer.from(expected));
}

/** Process-only, bounded sessions. Cookies have no authentication authority. */
export function browserAuth(config: WebBootstrapConfig) {
  if (!secretPattern.test(config.browserLaunchToken)) throw new Error('Invalid browser launch credential');
  const token = readFileSync(config.transportTokenFile, 'utf8');
  if (!/^[A-Za-z0-9_-]{43,128}$/.test(token) || token === config.browserLaunchToken) throw new Error('Invalid separated transport credential');
  const sessions = new Map<string, string>();
  return (request: IncomingMessage, response: ServerResponse, next: () => void) => {
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('Referrer-Policy', 'no-referrer');
    response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
    const authority = `127.0.0.1:${request.socket.localPort}`;
    const deny = (status = 401) => { response.writeHead(status, { 'Content-Type': 'text/plain' }).end('rustX browser authentication required. Reopen the startup URL.'); };
    if (request.headers.host !== authority || (request.headers.origin && request.headers.origin !== `http://${authority}`)) return deny(403);
    const url = new URL(request.url ?? '/', `http://${authority}`);
    const origin = `http://${authority}`;
    if (url.searchParams.has('token')) {
      if (request.method !== 'GET' || url.pathname !== '/' || [...url.searchParams].length !== 1 || !matches(url.searchParams.get('token') ?? undefined, config.browserLaunchToken)) return deny();
      if (sessions.size >= 128) return deny(429);
      let proof: string;
      do { proof = randomBytes(32).toString('base64url'); } while (proof === config.browserLaunchToken || proof === token || sessions.has(proof));
      sessions.set(proof, origin);
      const script = `try{sessionStorage.setItem(${JSON.stringify(BROWSER_SESSION_STORAGE)},${JSON.stringify(proof)});location.replace('/');}catch{document.getElementById('status').textContent='Browser session storage is required. Enable it and reopen the startup URL.';}`;
      const hash = createHash('sha256').update(script).digest('base64');
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8',
        'Content-Security-Policy': `default-src 'none'; script-src 'sha256-${hash}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
        'X-Content-Type-Options': 'nosniff',
      }).end(`<!doctype html><meta charset="utf-8"><title>rustX browser authentication</title><p id="status">Opening rustX…</p><script>${script}</script>`);
      return;
    }
    const sensitive = url.pathname === '/__rustx/bootstrap' || url.pathname === '/product-host' || url.pathname.startsWith('/product-host/');
    if (!sensitive) return next();
    const proof = request.headers[BROWSER_SESSION_HEADER.toLowerCase()];
    if (typeof proof !== 'string' || !secretPattern.test(proof) || sessions.get(proof) !== origin) return deny();
    if (request.headers['sec-fetch-site'] === 'cross-site') return deny(403);
    if (url.pathname === '/__rustx/bootstrap') {
      if (request.method !== 'GET' || url.search) return deny(405);
      response.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({
        connectionMode: 'local', appServerEndpoint: config.appServerEndpoint, appServerTransportToken: token,
      }));
      return;
    }
    next();
  };
}
