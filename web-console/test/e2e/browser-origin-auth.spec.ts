import { test, expect } from '@playwright/test';
import { createServer, request, type Server } from 'node:http';
import { randomBytes } from 'node:crypto';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { browserAuth } from '../../host/browser-auth';
import { BROWSER_SESSION_HEADER, BROWSER_SESSION_STORAGE } from '../../browser-session';

async function listen(server: Server) {
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw Error('No listener');
  return `http://127.0.0.1:${address.port}`;
}
test('another loopback port cannot capture and replay browser authentication to the carrier', async ({ page }) => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-origin-auth-'));
  const launch = randomBytes(32).toString('base64url'), token = randomBytes(32).toString('base64url');
  const transportTokenFile = join(directory, 'token'); writeFileSync(transportTokenFile, token, { mode: 0o600 });
  const config = { appServerEndpoint: 'ws://127.0.0.1:1/', browserLaunchToken: launch, transportTokenFile };
  let auth = browserAuth(config);
  const a = createServer((req, res) => auth(req, res, () => res.writeHead(200, { 'Content-Type': 'text/html' }).end('<!doctype html><title>rustX auth boundary</title><h1>Carrier shell</h1>')));
  let capture!: (headers: string[]) => void;
  const observed = new Promise<string[]>(resolve => { capture = resolve; });
  const b = createServer((req, res) => { capture(req.rawHeaders); res.writeHead(200, { 'Content-Type': 'text/html' }).end('<!doctype html><title>Other loopback service</title><h1>Capture service</h1>'); });
  try {
    const originA = await listen(a), originB = await listen(b);
    await page.goto(`${originA}/?token=${launch}`); await expect(page).toHaveURL(`${originA}/`);
    await expect(page).toHaveTitle('rustX auth boundary'); await expect(page.getByRole('heading', { name: 'Carrier shell' })).toBeVisible();
    // Cookie implementation would also pass this initial request; the attack
    // assertion below must reject what the OTHER server actually observes.
    const admitted = await page.evaluate(async ({ key, header }) => {
      const proof = sessionStorage.getItem(key);
      return (await fetch('/__rustx/bootstrap', { headers: proof ? { [header]: proof } : {} })).status;
    }, { key: BROWSER_SESSION_STORAGE, header: BROWSER_SESSION_HEADER });
    expect(admitted).toBe(200);
    await page.goto(`${originB}/capture`); await expect(page.getByRole('heading', { name: 'Capture service' })).toBeVisible();
    const rawHeaders = await observed;
    const stolen: Record<string, string> = {};
    for (let index = 0; index < rawHeaders.length; index += 2) stolen[rawHeaders[index].toLowerCase()] = rawHeaders[index + 1];
    // Attacker knows A's address and may choose Host or omit Origin; no secret
    // is injected. All credential-bearing inputs come only from B's capture.
    stolen.host = new URL(originA).host; delete stolen.origin;
    const attack = await new Promise<{ status?: number; body: string }>((resolve, reject) => {
      const req = request(`${originA}/__rustx/bootstrap`, { headers: stolen }, res => {
        let body = ''; res.on('data', chunk => { body += String(chunk); }); res.on('end', () => resolve({ status: res.statusCode, body }));
      }); req.on('error', reject); req.end();
    });
    expect(attack.status, 'replay using only actual cross-port browser headers must fail').toBe(401);
    expect(attack.body).not.toContain(token);
    expect(stolen.cookie).toBeUndefined(); expect(stolen[BROWSER_SESSION_HEADER.toLowerCase()]).toBeUndefined();
    expect(await page.evaluate(() => Object.keys(sessionStorage))).toEqual([]);
    await page.goto(`${originA}/`);
    const storage = await page.evaluate(() => ({ local: { ...localStorage }, session: { ...sessionStorage }, cookie: document.cookie, url: location.href }));
    const proof = storage.session[BROWSER_SESSION_STORAGE];
    expect(proof).toMatch(/^[A-Za-z0-9_-]{43}$/); expect([launch, token]).not.toContain(proof);
    expect(Object.keys(storage.session)).toEqual([BROWSER_SESSION_STORAGE]); expect(storage.local).toEqual({});
    expect(JSON.stringify(storage)).not.toContain(launch); expect(JSON.stringify(storage)).not.toContain(token);
    expect(JSON.stringify(rawHeaders)).not.toContain(proof); expect(storage.cookie).toBe(''); expect(storage.url).toBe(`${originA}/`);
    expect(await page.context().cookies()).toEqual([]); expect(await page.evaluate(() => indexedDB.databases())).toEqual([]);
    auth = browserAuth(config); // Same authority, fresh carrier activation; old tab storage survives.
    expect(await page.evaluate(async ({ key, header }) => (await fetch('/__rustx/bootstrap', { headers: { [header]: sessionStorage.getItem(key)! } })).status,
      { key: BROWSER_SESSION_STORAGE, header: BROWSER_SESSION_HEADER })).toBe(401);
    await page.goto(`${originA}/?token=${launch}`); await expect(page).toHaveURL(`${originA}/`);
    expect(await page.evaluate(key => sessionStorage.getItem(key), BROWSER_SESSION_STORAGE)).not.toBe(proof);
  } finally {
    await page.close();
    await Promise.all([a, b].map(server => new Promise<void>(resolve => server.close(() => resolve()))));
    rmSync(directory, { recursive: true, force: true });
  }
});
