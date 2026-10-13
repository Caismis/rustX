import { afterEach, expect, it } from 'vitest';
import { waitFor } from 'xstate';
import { Server, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import type { ShellNavigationActor } from '../src/app/navigation/machine';

const cleanups: (() => void)[] = [];
afterEach(() => { cleanups.splice(0).forEach(stop => stop()); });
async function fixture() {
  const server = new Server(); await server.attached('A', 'B'); server.snapshots.set('C', snapshot('C'));
  const f = routeFixture(server.client, { ...server.workspaceHost, classifyLocations: async cwds => cwds.map(() => ({ authorized: true })) });
  cleanups.push(() => { f.stop(); server.client.disconnect(); });
  return { ...f, server };
}
const idle = (actor: ShellNavigationActor) => waitFor(actor, snapshot => snapshot.matches('idle'));
const mutations = (server: Server) => server.requests.filter(row => ['session/attach', 'session/detach', 'session/switchNode', 'turn/start'].includes(row.request.method)).map(row => row.request);
it('failed preparation preserves the committed route, attachment and controls without refresh', async () => {
  const f = await fixture(), controls = f.capture(), before = f.actor.getSnapshot().context.route, target = f.server.client.target('A');
  const count = mutations(f.server).length, snapshots = f.server.requests.filter(row => row.request.method === 'session/snapshot').length;
  f.server.held.add('session/attach'); f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  expect(controls()).toBe(true); expect(f.server.client.target('A')).toBe(target);
  f.server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'refused', data: { kind: 'operation_failed' } } });
  await idle(f.actor); expect(f.actor.getSnapshot().context.route).toBe(before); expect(controls()).toBe(true);
  expect(mutations(f.server)).toHaveLength(count + 1); expect(f.server.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(snapshots);
  expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority']);
});
it('an authoritatively attached destination commits synchronously and supersedes held cold settlement', async () => {
  const f = await fixture(); f.server.held.add('session/attach');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: true } });
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'B' });
  f.server.reply(request); await f.server.client.waitForAttachment('C', () => true);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'B' }); expect(f.actor.getSnapshot().context.error).toBeUndefined();
  expect(mutations(f.server).filter(request => request.method === 'session/attach')).toHaveLength(3);
  expect(f.server.client.isAttachmentControlCurrent('C', f.server.client.getSnapshot().views.C.attachmentObservation)).toBe(true);
});
for (const target of [userSettingsTarget, workspaceSettingsTarget('project', 'rustX')]) it(`Settings ${target.kind} retires admitted preparation while native attachment settles independently`, async () => {
  const f = await fixture(), controls = f.capture(); f.server.held.add('session/attach');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target } });
  expect(f.settings.getSnapshot().context.target).toEqual(target); expect(controls()).toBe(true);
  f.server.reply(request); await f.server.client.waitForAttachment('C', () => true);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' }); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(controls()).toBe(true);
  expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority']);
});
it('idle Settings and overlay dismissal preserve committed controls; publication replaces only old controls', async () => {
  const f = await fixture(), controls = f.capture();
  f.actor.send({ type: 'SIDEBAR.DISMISS' }); f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target: userSettingsTarget } }); f.settings.send({ type: 'CLOSE' });
  expect(controls()).toBe(true);
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: true } });
  const next = f.capture(); f.actor.send({ type: 'SIDEBAR.DISMISS' }); expect(next()).toBe(true); expect(controls()).toBe(false);
});
it('authority replacement retires preparation and old controls without admitted-operation replay', async () => {
  const f = await fixture(), controls = f.capture(); f.server.held.add('session/attach');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  f.actor.send({ type: 'AUTHORITY', generation: f.server.client.getSnapshot().generation, authority: (f.server.client.getSnapshot().authorityRevision ?? 0) + 1 });
  f.server.reply(request); await f.server.client.waitForAttachment('C', () => true);
  expect(controls()).toBe(false); expect(f.actor.getSnapshot().context.route.kind).toBe('new-conversation'); expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority']);
});

it.each(['Settings', 'newer Session', 'newer cold Session'] as const)('native draft creation keeps settlement while %s supersedes its route publication', async destination => {
  const f = await fixture(); f.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  const control = f.actor.getSnapshot().context.control, generation = f.server.client.getSnapshot().generation, binding = 'creation-owned';
  const { firstSubmitPort } = await import('../src/app/new-conversation/port');
  const host = { ...f.server.workspaceHost, resolveWorkspace: async () => ({ cwd: '/workspace/A' }) };
  f.server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'A-created', active_node: 'created-node', active_conversation_id: 'created-conversation', node_count: 1, created_at: '0', updated_at: '0' } }));
  f.server.summaries.set('A-created', { cwd: '/workspace/A' }); f.server.snapshots.set('A-created', { ...snapshot('A-created'), conversation_id: 'created-conversation' }); f.server.nodeSnapshots.set('created-node', f.server.snapshots.get('A-created')!);
  f.server.held.add('session/create');
  const port = firstSubmitPort(f.server.client, host, f.capture(), () => {
    f.actor.send({ type: 'CREATE', control, binding }); const pending = f.actor.getSnapshot().context.pending;
    return sessionId => { f.actor.send({ type: 'CREATED', sessionId, control, generation, pending }); return f.capture(); };
  });
  const work = f.server.client.firstSubmissions.submit(binding, { workspaceId: 'workspace-a', text: 'Native execution intent', files: [] }, port);
  const request = await f.server.waitFor('session/create', 1);
  if (destination === 'Settings') f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target: userSettingsTarget } });
  else {
    if (destination === 'newer cold Session') f.server.held.add('session/attach');
    f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: destination === 'newer cold Session' ? 'C' : 'B', sidebar: false } });
  }
  const cold = destination === 'newer cold Session' ? await f.server.waitFor('session/attach', 3) : undefined;
  f.server.reply(request); await work;
  if (cold) {
    expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'C' });
    expect(f.actor.getSnapshot().context.pending?.destination).toMatchObject({ kind: 'session', sessionId: 'C' });
    expect(f.server.requests.filter(row => row.request.method === 'session/attach' && row.request.params.session_id === 'A-created')).toHaveLength(0);
    f.server.reply(cold); await idle(f.actor);
  }
  expect(f.server.requests.filter(row => row.request.method === 'session/create')).toHaveLength(1);
  expect(f.actor.getSnapshot().context.route).toEqual(destination === 'Settings' ? { kind: 'new-conversation', workspaceId: 'workspace-a' } : { kind: 'session', sessionId: destination === 'newer cold Session' ? 'C' : 'B' });
  expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority']);
  expect(f.server.client.firstSubmissions.draft(binding)?.session?.id).toBe('A-created');
  expect(f.server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(destination === 'Settings' ? 1 : 0);
});
