// @vitest-environment node
import { afterEach, expect, it } from 'vitest';
import { createServer, request, type Server } from 'node:http';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { browserAuth } from '../host/browser-auth';
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
    const response = await fetch(`${url}/?token=${launch}`, { redirect: 'manual' });
    expect(response.status).toBe(303); expect(response.headers.get('location')).toBe('/');
    expect(response.headers.get('cache-control')).toBe('no-store');
    const cookie = response.headers.get('set-cookie')!;
    expect(cookie).toMatch(/; HttpOnly; SameSite=Strict; Path=\/$/);
    expect(cookie).not.toMatch(/Domain|Expires|Max-Age/);
    return cookie.split(';')[0];
  };
  return { url, login, directory, restart: () => { auth = browserAuth(config); } };
}
it('root-only exchange redirects before resources; exact authenticated bootstrap is not cacheable', async () => {
  const c = await carrier(), cookie = await c.login();
  const response = await fetch(`${c.url}/__rustx/bootstrap`, { headers: { cookie } });
  expect(response.status).toBe(200); expect(response.headers.get('cache-control')).toBe('no-store');
  expect(await response.json()).toEqual({ connectionMode: 'local', appServerEndpoint: 'ws://127.0.0.1:3456/', appServerTransportToken: transport });
  expect(await (await fetch(`${c.url}/`, { headers: { cookie } })).text()).toBe('application');
  expect((await fetch(`${c.url}/product-host/adopt`, { method: 'POST', headers: { cookie, 'content-type': 'application/json' }, body: JSON.stringify({ location: `${c.directory}/unauthorized` }) })).status).toBe(400);
});
it('fails closed for absent, malformed, wrong, duplicate and native tokens, on every protected route', async () => {
  const c = await carrier();
  for (const path of ['/', '/__rustx/bootstrap', '/product-host/list', '/?token=', '/?token=short', `/?token=${transport}`, `/?token=${'X'.repeat(43)}`, `/?token=${launch}&token=${launch}`, `/__rustx/bootstrap?token=${launch}`, `/src/main.tsx?token=${launch}`]) {
    const response = await fetch(c.url + path, { redirect: 'manual', headers: { authorization: `Bearer ${launch}` } });
    expect(response.status, path).toBe(401);
  }
  const cookie = await c.login();
  for (const invalid of [cookie + '; ' + cookie, cookie + 'x', cookie.replace(/=.*/, '=bad')]) expect((await fetch(`${c.url}/__rustx/bootstrap`, { headers: { cookie: invalid } })).status).toBe(401);
});
it('binds session to authority including port and rejects it after carrier rotation', async () => {
  const a = await carrier(), b = await carrier(), cookie = await a.login();
  expect((await fetch(`${b.url}/__rustx/bootstrap`, { headers: { cookie } })).status).toBe(401);
  const bCookie = await b.login();
  const renamed = `${bCookie.split('=')[0]}=${cookie.split('=')[1]}`;
  expect((await fetch(`${b.url}/__rustx/bootstrap`, { headers: { cookie: renamed } })).status).toBe(401);
  const wrongHost = await new Promise<number | undefined>((resolve, reject) => {
    const req = request(`${a.url}/__rustx/bootstrap`, { headers: { cookie, host: new URL(b.url).host } }, res => { res.resume(); res.on('end', () => resolve(res.statusCode)); });
    req.on('error', reject); req.end();
  });
  expect(wrongHost).toBe(403);
  expect((await fetch(`${a.url}/__rustx/bootstrap`, { headers: { cookie, origin: b.url } })).status).toBe(403);
  a.restart();
  expect((await fetch(`${a.url}/__rustx/bootstrap`, { headers: { cookie } })).status).toBe(401);
});
it('even with the same activation secret, a renamed cookie cannot authenticate another port', async () => {
  const a = await carrier(), cookie = await a.login();
  const shared = createServer(servers[0].listeners('request')[0] as Parameters<typeof createServer>[0]);
  servers.push(shared);
  await new Promise<void>(resolve => shared.listen(0, '127.0.0.1', resolve));
  const address = shared.address(); if (!address || typeof address === 'string') throw Error('No address');
  const origin = `http://127.0.0.1:${address.port}`;
  const login = await fetch(`${origin}/?token=${launch}`, { redirect: 'manual' });
  const name = login.headers.get('set-cookie')!.split('=')[0];
  expect((await fetch(`${origin}/__rustx/bootstrap`, { headers: { cookie: `${name}=${cookie.split('=')[1]}` } })).status).toBe(401);
});
