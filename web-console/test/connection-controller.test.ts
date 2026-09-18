import { expect, it, vi } from 'vitest';
import { ConnectionController } from '../src/connection/controller';
import { Server, TOKEN, endpoint } from './fixture';
function bootstrap() { return Promise.resolve(new Response(JSON.stringify({ connectionMode: 'local', appServerEndpoint: endpoint, appServerTransportToken: TOKEN }), { headers: { 'content-type': 'application/json' } })); }
it('local bootstrap is the only local material source; no remembered remote fallback', async () => {
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'wss://remembered.example/' }));
  const server = new Server(), fetcher = vi.fn(bootstrap), owner = new ConnectionController(server.client, fetcher);
  await owner.start(); expect(server.sockets).toHaveLength(1);
  await owner.reconnect(); expect(server.sockets).toHaveLength(2); expect(server.sockets[0].closed).toBe(true);
  await owner.disconnect();
  fetcher.mockRejectedValueOnce(Error('offline'));
  await owner.reconnect();
  expect(owner.getSnapshot().mode).toBe('local'); expect(server.sockets).toHaveLength(2);
  expect(owner.getSnapshot().error).toContain('No local managed');
});
it('selecting Remote only opens material entry; ownership commits after exactly one close', async () => {
  const server = new Server(), fetcher = vi.fn(bootstrap), owner = new ConnectionController(server.client, fetcher);
  await owner.start();
  const old = server.socket, close = vi.spyOn(old, 'close').mockImplementation(() => {});
  await owner.select('remote');
  expect(owner.getSnapshot().mode).toBe('local'); expect(close).not.toHaveBeenCalled();
  expect(owner.getSnapshot().selectedMode).toBe('remote');
  const connecting = owner.connectRemote('wss://remote.example/', TOKEN);
  expect(owner.getSnapshot().mode).toBe('local'); expect(close).toHaveBeenCalledTimes(1);
  expect(server.sockets).toHaveLength(1);
  old.onclose?.(new CloseEvent('close')); await connecting;
  expect(owner.getSnapshot().mode).toBe('remote'); expect(server.sockets).toHaveLength(2);
  const remote = server.socket, remoteClose = vi.spyOn(remote, 'close');
  await owner.select('local');
  expect(fetcher).toHaveBeenCalledTimes(2); expect(server.sockets).toHaveLength(3);
  expect(remoteClose).toHaveBeenCalledTimes(1); expect(owner.getSnapshot().mode).toBe('local');
  await owner.disconnect();
});
it('invalid remote inputs use client validation and never activate local mode', async () => {
  const server = new Server(), fetcher = vi.fn(bootstrap), owner = new ConnectionController(server.client, fetcher);
  await owner.select('remote');
  for (const url of ['http://example.com/', 'ws://user:pass@example.com/', 'wss://example.com/path', 'ws://example.com/?token=x', 'ws://example.com/#x']) await owner.connectRemote(url, TOKEN);
  await owner.connectRemote(endpoint, 'bad');
  expect(server.sockets).toHaveLength(0); expect(fetcher).not.toHaveBeenCalled(); expect(owner.getSnapshot().mode).toBe('local');
  await owner.connectRemote('wss://remote.example/', TOKEN); expect(server.sockets).toHaveLength(1);
  expect(JSON.stringify({ ...localStorage })).not.toContain(TOKEN); expect(JSON.stringify({ ...sessionStorage })).not.toContain(TOKEN);
  await owner.disconnect();
});
it('late bootstrap cannot replace an explicitly selected remote connection', async () => {
  let release!: (response: Response) => void;
  const server = new Server(), owner = new ConnectionController(server.client, () => new Promise(resolve => { release = resolve; }));
  const starting = owner.start(); await owner.select('remote'); await owner.connectRemote(endpoint, TOKEN);
  release(await bootstrap()); await starting;
  expect(server.sockets).toHaveLength(1); expect(owner.getSnapshot().mode).toBe('remote'); await owner.disconnect();
});
it('competing replacements behind one close barrier create only the newest socket', async () => {
  const server = new Server(); await server.connect();
  const old = server.socket, close = vi.spyOn(old, 'close').mockImplementation(() => {});
  const first = server.client.connect('wss://first.example/', TOKEN, 'replace-authority');
  const second = server.client.connect('wss://second.example/', TOKEN, 'replace-authority');
  expect(server.sockets).toHaveLength(1); expect(close).toHaveBeenCalledTimes(1);
  old.onclose?.(new CloseEvent('close')); await Promise.all([first, second]);
  expect(server.sockets).toHaveLength(2); expect(server.client.getSnapshot().endpoint).toBe('wss://second.example/');
  await server.client.disconnect();
});

it('invalid replacement material leaves the active Local connection untouched', async () => {
  const server = new Server(), owner = new ConnectionController(server.client, bootstrap);
  await owner.start(); await owner.select('remote');
  const before = server.client.getSnapshot(), close = vi.spyOn(server.socket, 'close');
  await owner.connectRemote('https://remote.example/', TOKEN);
  expect(owner.getSnapshot().error).toContain('Use a ws://');
  await owner.connectRemote('wss://remote.example/', 'bad');
  expect(owner.getSnapshot().error).toContain('transport token');
  expect(close).not.toHaveBeenCalled(); expect(server.sockets).toHaveLength(1);
  expect(server.client.getSnapshot()).toBe(before); expect(owner.getSnapshot().mode).toBe('local');
  await server.client.listSessions(); await owner.disconnect();
});
