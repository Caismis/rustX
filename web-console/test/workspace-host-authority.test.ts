// @vitest-environment node
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { AppServerTransport } from '../../tui/src/app-server/transport.ts';
import { cfg3Source } from './cfg3-data';
import { LocalWorkspaceHost } from '../host/workspaces';

interface ObservedRequest { id: string | number; method: string; params: unknown }
const native = vi.hoisted(() => {
  const state = {
    requests: [] as ObservedRequest[],
    waiters: [] as (() => void)[],
    connects: 0,
    transport: undefined as FakeTransport | undefined,
  };
  class FakeTransport implements AppServerTransport {
    readonly closed = undefined;
    private listeners = new Set<(record: unknown) => void>();
    describe() { return 'fake'; }
    send(message: unknown) {
      const request = message as ObservedRequest;
      state.requests.push(request);
      for (const waiter of state.waiters.splice(0)) waiter();
      if (request.method === 'initialize') queueMicrotask(() => this.deliver({ jsonrpc: '2.0', id: request.id, result: { type: 'initialized', protocol_version: 18, capabilities: { multi_session: true, single_writable_controller: true, headless_interactions: true, experimental_methods: [] } } }));
      return Promise.resolve();
    }
    onMessage(listener: (record: unknown) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
    onClose() { return () => {}; }
    close() {}
    deliver(record: unknown) { for (const listener of [...this.listeners]) listener(record); }
  }
  return { state, FakeTransport };
});
vi.mock('../../tui/src/app-server/websocket-transport.ts', () => ({
  WebSocketTransport: { connect: async () => { native.state.connects++; native.state.transport = new native.FakeTransport(); return native.state.transport; } },
}));

const directories: string[] = [];
afterEach(() => { directories.splice(0).forEach(path => rmSync(path, { force: true, recursive: true })); });
beforeEach(() => { native.state.requests.length = 0; native.state.waiters.length = 0; native.state.connects = 0; native.state.transport = undefined; });
function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'workspace-authority-')); directories.push(directory);
  const a = join(directory, 'A'); mkdirSync(a);
  const config = { endpoint: 'ws://127.0.0.1:8080/', picker: true, transportToken: 'transport-token', metadataFile: join(directory, 'registrations.json'), roots: [{ id: 'a', cwd: a, displayName: 'Alpha' }] };
  return { config, host: new LocalWorkspaceHost(config), endpoint: config.endpoint };
}
async function awaitMethod(method: string, count = 1) {
  while (native.state.requests.filter(request => request.method === method).length < count) await new Promise<void>(resolve => native.state.waiters.push(resolve));
  return native.state.requests.filter(request => request.method === method);
}
const tick = () => new Promise<void>(resolve => setImmediate(resolve));
const write = { kind: 'write' as const, expected_revision: 'workspace-1', mutation: { kind: 'config' as const, mutation: { unit: 'native_tools' as const, authored: ['read'] } } };
const answer = (request: ObservedRequest) => native.state.transport!.deliver({ jsonrpc: '2.0', id: request.id, result: { type: 'source_settings', projection: cfg3Source() } });

it('mutation wins: an in-flight configuration mutation completes before removal commits', async () => {
  const { host, config, endpoint } = fixture();
  const [alpha] = (await host.listWorkspaces()).workspaces;
  const configuration = host.configureWorkspace(alpha.id, endpoint, write);
  const [submitted] = await awaitMethod('configuration/sourceWrite');
  let removed = false;
  const removal = host.removeWorkspace(alpha.id).then(() => { removed = true; });
  await tick();
  expect(removed).toBe(false);
  expect((await host.listWorkspaces()).workspaces.map(row => row.id)).toContain(alpha.id);
  expect(readFileSync(config.metadataFile, 'utf8')).toContain(alpha.id);
  answer(submitted);
  const [reread] = await awaitMethod('configuration/sourcesRead');
  answer(reread);
  await expect(configuration).resolves.toEqual({ kind: 'write', commit: { acknowledgement: cfg3Source(), reread: { status: 'observed', projection: cfg3Source() } } });
  expect(native.state.requests.filter(request => request.method === 'configuration/sourceWrite')).toHaveLength(1);
  expect(native.state.requests.filter(request => request.method === 'configuration/reconcile')).toHaveLength(0);
  await removal;
  expect(removed).toBe(true);
  expect((await host.listWorkspaces()).workspaces.map(row => row.id)).not.toContain(alpha.id);
  expect(readFileSync(config.metadataFile, 'utf8')).not.toContain(alpha.id);
});

it('revoke wins: a removal in flight fences a concurrently submitted configuration mutation before any native request', async () => {
  const { host, config, endpoint } = fixture();
  const [alpha] = (await host.listWorkspaces()).workspaces;
  let removed = false;
  const removal = host.removeWorkspace(alpha.id).then(() => { removed = true; });
  // Lane bodies run on a microtask even for the first caller, so the mutation
  // submitted here provably queues behind the in-flight, still-uncommitted removal.
  const configuration = host.configureWorkspace(alpha.id, endpoint, write);
  expect(removed).toBe(false);
  expect(native.state.connects).toBe(0);
  expect(readFileSync(config.metadataFile, 'utf8')).toContain(alpha.id);
  await removal;
  expect(removed).toBe(true);
  expect((await host.listWorkspaces()).workspaces.map(row => row.id)).not.toContain(alpha.id);
  expect(readFileSync(config.metadataFile, 'utf8')).not.toContain(alpha.id);
  // Had the mutation entered the lane first it would have connected before the
  // removal could commit; rejection with zero native requests proves it ran after.
  await expect(configuration).rejects.toThrow('Unknown Workspace registration');
  await expect(host.configureWorkspace(alpha.id, endpoint, write)).rejects.toThrow('Unknown Workspace registration');
  expect(native.state.connects).toBe(0);
  expect(native.state.requests.filter(request => request.method === 'configuration/sourceWrite')).toHaveLength(0);
  expect(native.state.requests.filter(request => request.method === 'configuration/reconcile')).toHaveLength(0);
  expect(native.state.requests).toHaveLength(0);
});

it('a confirmed write keeps its acknowledgement when the post-write reread fails', async () => {
  const { host, endpoint } = fixture();
  const [alpha] = (await host.listWorkspaces()).workspaces;
  const configuration = host.configureWorkspace(alpha.id, endpoint, write);
  const [submitted] = await awaitMethod('configuration/sourceWrite');
  native.state.transport!.deliver({ jsonrpc: '2.0', id: submitted.id, result: { type: 'source_settings', projection: cfg3Source() } });
  const [reread] = await awaitMethod('configuration/sourcesRead');
  // The authoritative reread fails after the write already committed. The
  // operation still resolves, and reports the failure as a separate fact.
  native.state.transport!.deliver({ jsonrpc: '2.0', id: reread.id, error: { code: -32000, message: 'native reread failed' } });
  const result = await configuration;
  expect(result.kind).toBe('write');
  if (result.kind !== 'write') throw new Error('expected a write outcome');
  expect(result.commit.acknowledgement).toEqual(cfg3Source());
  expect(result.commit.reread.status).toBe('failed');
  expect(String((result.commit.reread as { error: unknown }).error)).toContain('native reread failed');
  // Exactly one write reached native; the failure never caused a replay.
  expect(native.state.requests.filter(request => request.method === 'configuration/sourceWrite')).toHaveLength(1);
});
