import { createHash, createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import type { IncomingMessage, ServerResponse } from 'node:http';

export interface WebBootstrapConfig { appServerEndpoint: string; transportTokenFile: string; browserLaunchToken: string }
const secretPattern = /^[A-Za-z0-9_-]{43}$/;
function matches(actual: string | undefined, expected: string) {
  return actual !== undefined && secretPattern.test(actual) && timingSafeEqual(Buffer.from(actual), Buffer.from(expected));
}

/** One carrier activation, one ephemeral session secret. No native/Workspace authority. */
export function browserAuth(config: WebBootstrapConfig) {
  if (!secretPattern.test(config.browserLaunchToken)) throw new Error('Invalid browser launch credential');
  const token = readFileSync(config.transportTokenFile, 'utf8');
  if (!/^[A-Za-z0-9_-]{43,128}$/.test(token) || token === config.browserLaunchToken) throw new Error('Invalid separated transport credential');
  const secret = randomBytes(32);
  return (request: IncomingMessage, response: ServerResponse, next: () => void) => {
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('Referrer-Policy', 'no-referrer');
    const authority = `127.0.0.1:${request.socket.localPort}`;
    const deny = (status = 401) => { response.writeHead(status, { 'Content-Type': 'text/plain' }).end('rustX browser authentication required. Reopen the startup URL.'); };
    if (request.headers.host !== authority || (request.headers.origin && request.headers.origin !== `http://${authority}`)) return deny(403);
    const url = new URL(request.url ?? '/', `http://${authority}`);
    const name = `rustx-browser-${createHash('sha256').update(authority).digest('hex').slice(0, 16)}`;
    const session = createHmac('sha256', secret).update(authority).digest('base64url');
    if (url.searchParams.has('token')) {
      if (request.method !== 'GET' || url.pathname !== '/' || [...url.searchParams].length !== 1 || !matches(url.searchParams.get('token') ?? undefined, config.browserLaunchToken)) return deny();
      response.writeHead(303, { Location: '/', 'Set-Cookie': `${name}=${session}; HttpOnly; SameSite=Strict; Path=/` }).end();
      return;
    }
    const cookies = (request.headers.cookie ?? '').split(';').map(part => part.trim()).filter(part => part.startsWith(`${name}=`));
    if (cookies.length !== 1 || !matches(cookies[0].slice(name.length + 1), session)) return deny();
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
