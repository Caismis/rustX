// @vitest-environment node
import { afterEach, expect, it } from 'vitest';
import { createServer, request, type Server } from 'node:http';
import { createHash } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { browserAuth } from '../host/browser-auth';
import { BROWSER_SESSION_HEADER, BROWSER_SESSION_STORAGE } from '../browser-session';
import { workspaceHandler } from '../host/http';
import { LocalWorkspaceHost } from '../host/workspaces';

const launch = 'L'.repeat(43), transport = 'T'.repeat(43);
const servers: Server[] = [], directories: string[] = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map(server => new Promise<void>(resolve => server.close(() => resolve()))));
  directories.splice(0).forEach(path => rmSync(path, { recursive: true, force: true }));
});
async function carrier() {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-auth-test-')); directories.push(directory);
  const tokenFile = join(directory, 'token'); writeFileSync(tokenFile, transport, { mode: 0o600 });
  const config = { appServerEndpoint: 'ws://127.0.0.1:3456/', transportTokenFile: tokenFile, browserLaunchToken: launch };
  let auth = browserAuth(config);
  const host = workspaceHandler(new LocalWorkspaceHost({ endpoint: config.appServerEndpoint, metadataFile: join(directory, 'workspaces.json'), picker: true, roots: [{ id: 'root', cwd: directory, displayName: 'Root' }] }));
  const server = createServer((req, res) => auth(req, res, () => { void host(req, res, () => res.end('application')); }));
  servers.push(server);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw Error('No address');
  const url = `http://127.0.0.1:${address.port}`;
  const login = async () => {
    const response = await fetch(`${url}/?token=${launch}`);
    expect(response.status).toBe(200); expect(response.headers.get('set-cookie')).toBeNull();
    expect(response.headers.get('cache-control')).toBe('no-store'); expect(response.headers.get('referrer-policy')).toBe('no-referrer');
    const html = await response.text(), script = html.match(/<script>(.*?)<\/script>/)![1];
    expect(html).not.toContain(launch); expect(html).not.toContain(transport); expect(html).not.toMatch(/src=|href=/);
    expect(response.headers.get('content-security-policy')).toBe(`default-src 'none'; script-src 'sha256-${createHash('sha256').update(script).digest('base64')}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`);
    const stored = new Map<string, string>(), navigation: string[] = [];
    runInNewContext(script, { sessionStorage: { setItem: (key: string, value: string) => stored.set(key, value) }, location: { replace: (path: string) => navigation.push(path) } });
    expect([...stored.keys()]).toEqual([BROWSER_SESSION_STORAGE]); expect(navigation).toEqual(['/']);
    const proof = stored.get(BROWSER_SESSION_STORAGE)!;
    expect(proof).toMatch(/^[A-Za-z0-9_-]{43}$/); expect([launch, transport]).not.toContain(proof);
    return proof;
  };
  return { url, login, directory, restart: () => { auth = browserAuth(config); } };
}
it('resource-free exchange mints fresh separated proofs and replaces the credential URL; bootstrap stays narrow', async () => {
  const c = await carrier(), proof = await c.login(); expect(await c.login()).not.toBe(proof);
  const response = await fetch(`${c.url}/__rustx/bootstrap`, { headers: { [BROWSER_SESSION_HEADER]: proof } });
  expect(response.status).toBe(200); expect(response.headers.get('cache-control')).toBe('no-store');
  expect(await response.json()).toEqual({ connectionMode: 'local', appServerEndpoint: 'ws://127.0.0.1:3456/', appServerTransportToken: transport });
  expect(await (await fetch(`${c.url}/`)).text()).toBe('application');
});
it('rejects malformed/wrong/native/duplicate launch credentials and non-root exchanges', async () => {
  const c = await carrier();
  for (const path of ['/?token=', '/?token=short', `/?token=${transport}`, `/?token=${'X'.repeat(43)}`, `/?token=${launch}&token=${launch}`, `/__rustx/bootstrap?token=${launch}`, `/src/main.tsx?token=${launch}`]) {
    expect((await fetch(c.url + path, { headers: { authorization: `Bearer ${launch}` } })).status, path).toBe(401);
  }
});
it('both sensitive API families reject absent/malformed/wrong proofs, Cookies and launch/native bearers', async () => {
  const c = await carrier();
  for (const path of ['/__rustx/bootstrap', '/product-host/list']) for (const proof of ['', 'bad', 'X'.repeat(43), launch, transport]) {
    expect((await fetch(c.url + path, { headers: { [BROWSER_SESSION_HEADER]: proof, cookie: `rustx-browser-old=${proof}`, authorization: `Bearer ${proof}` } })).status).toBe(401);
  }
  const proof = await c.login();
  expect((await fetch(`${c.url}/__rustx/bootstrap`, { headers: { cookie: `rustx-browser-old=${proof}` } })).status).toBe(401);
  expect((await fetch(`${c.url}/__rustx/bootstrap`, { headers: { [BROWSER_SESSION_HEADER]: `${proof}, ${proof}` } })).status).toBe(401);
});
it('authenticated Product Host access retains exact-root and endpoint authorization', async () => {
  const c = await carrier(), proof = await c.login();
  const post = (method: string, body: unknown = {}) => fetch(`${c.url}/product-host/${method}`, { method: 'POST', headers: { [BROWSER_SESSION_HEADER]: proof, 'content-type': 'application/json' }, body: JSON.stringify(body) });
  expect((await post('list')).status).toBe(200);
  const classified = await post('classify', { endpoint: 'ws://127.0.0.1:3456/', cwds: [c.directory, `${c.directory}/child`, '/unauthorized'] });
  expect((await classified.json()).map((row: { authorized: boolean }) => row.authorized)).toEqual([true, false, false]);
  expect((await post('adopt', { location: `${c.directory}/unauthorized` })).status).toBe(400);
  expect((await post('classify', { endpoint: 'wss://remote.example/', cwds: [c.directory] })).status).toBe(400);
});
it('validates exact authority and rejects wrong Host/Origin and stale proof after restart', async () => {
  const a = await carrier(), b = await carrier(), proof = await a.login(), headers = { [BROWSER_SESSION_HEADER]: proof };
  expect((await fetch(`${b.url}/__rustx/bootstrap`, { headers })).status).toBe(401);
  const wrongHost = await new Promise<number | undefined>((resolve, reject) => {
    const req = request(`${a.url}/__rustx/bootstrap`, { headers: { ...headers, host: new URL(b.url).host } }, res => { res.resume(); res.on('end', () => resolve(res.statusCode)); }); req.on('error', reject); req.end();
  });
  expect(wrongHost).toBe(403); expect((await fetch(`${a.url}/__rustx/bootstrap`, { headers: { ...headers, origin: b.url } })).status).toBe(403);
  a.restart(); expect((await fetch(`${a.url}/__rustx/bootstrap`, { headers })).status).toBe(401);
  const fresh = await a.login(); expect(fresh).not.toBe(proof);
  expect((await fetch(`${a.url}/__rustx/bootstrap`, { headers: { [BROWSER_SESSION_HEADER]: fresh } })).status).toBe(200);
});
it('even a shared validator instance rejects its other-port proof', async () => {
  const a = await carrier(), proof = await a.login();
  const shared = createServer(servers[0].listeners('request')[0] as Parameters<typeof createServer>[0]); servers.push(shared);
  await new Promise<void>(resolve => shared.listen(0, '127.0.0.1', resolve));
  const address = shared.address(); if (!address || typeof address === 'string') throw Error('No address');
  expect((await fetch(`http://127.0.0.1:${address.port}/__rustx/bootstrap`, { headers: { [BROWSER_SESSION_HEADER]: proof } })).status).toBe(401);
});
it('session capacity fails closed without evicting an existing tab', async () => {
  const c = await carrier(), proof = await c.login();
  for (let index = 1; index < 128; index++) expect((await fetch(`${c.url}/?token=${launch}`)).status).toBe(200);
  expect((await fetch(`${c.url}/?token=${launch}`)).status).toBe(429);
  expect((await fetch(`${c.url}/__rustx/bootstrap`, { headers: { [BROWSER_SESSION_HEADER]: proof } })).status).toBe(200);
});
