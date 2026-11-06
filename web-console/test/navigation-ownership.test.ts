import { afterEach, expect, it, vi } from 'vitest';
import { waitFor } from 'xstate';
import { Server, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import type { ShellNavigationActor } from '../src/app/navigation/machine';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';

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
it('opens the original task node directly from the task page without first activating the Session default node', async () => {
  const f = await fixture(), original = f.server.client.target('A');
  const history = snapshot('task-origin'); f.server.nodeSnapshots.set('task-node', history);
  f.actor.send({ type: 'SCHEDULES' });
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', nodeId: 'task-node', conversationId: history.conversation_id, sidebar: false } });
  const attach = await f.server.waitFor('session/attach', 3);
  expect(attach.params).toMatchObject({ session_id: 'C', node_id: 'task-node' });
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'C' });
  expect(f.server.client.target('C')?.conversation_id).toBe(history.conversation_id);
  expect(f.server.client.target('A')).toBe(original);
  expect(mutations(f.server).some(request => request.method === 'session/switchNode')).toBe(false);
});
it('opens an original task node in an already attached Session through the native node transition and a fresh editor identity', async () => {
  const f = await fixture(), other = f.server.client.target('B'), binding = f.actor.getSnapshot().context.binding;
  const original = snapshot('task-original'); f.server.nodeSnapshots.set('task-original-node', original);
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'A', nodeId: 'task-original-node', conversationId: original.conversation_id, sidebar: false } });
  const switched = await f.server.waitFor('session/switchNode', 1);
  expect(switched.params).toMatchObject({ node_id: 'task-original-node' });
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' });
  expect(f.server.client.target('A')?.conversation_id).toBe(original.conversation_id);
  expect(f.actor.getSnapshot().context.binding).toBeGreaterThan(binding);
  expect(f.server.client.target('B')).toBe(other);
  expect(f.server.requests.some(row => row.request.method === 'turn/start' || row.request.method === 'turn/cancel')).toBe(false);
});
it('reselecting the same cold task node preserves its local editor identity while attachment is held', async () => {
  const f = await fixture(), original = snapshot('task-cold'); f.server.nodeSnapshots.set('task-cold-node', original);
  f.server.held.add('session/attach');
  const destination = { kind: 'session' as const, sessionId: 'C', nodeId: 'task-cold-node', conversationId: original.conversation_id, sidebar: false };
  f.actor.send({ type: 'OPEN', destination });
  const attached = await f.server.waitFor('session/attach', 3), binding = f.actor.getSnapshot().context.binding;
  f.actor.send({ type: 'OPEN', destination });
  expect(f.actor.getSnapshot().context.binding).toBe(binding);
  f.server.reply(attached); await idle(f.actor);
  expect(f.server.client.target('C')?.conversation_id).toBe(original.conversation_id);
  expect(f.actor.getSnapshot().context.binding).toBe(binding);
});
it('failed cold selection keeps its local draft and preserves previous native attachment authority without refresh', async () => {
  const f = await fixture(), controls = f.capture(), target = f.server.client.target('A'), proof = f.server.client.getSnapshot().views.A.attachmentObservation;
  const count = mutations(f.server).length, snapshots = f.server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  f.server.held.add('session/attach'); f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  expect(controls()).toBe(false); expect(f.server.client.target('A')).toBe(target);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'C' });
  f.server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'refused', data: { kind: 'operation_failed' } } });
  await idle(f.actor); expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'C' }); expect(controls()).toBe(false);
  expect(f.server.client.isAttachmentControlCurrent('A', proof)).toBe(true);
  expect(f.actor.getSnapshot().context.error).toContain('refused');
  expect(mutations(f.server)).toHaveLength(count + 1); expect(f.server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(snapshots);
  expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority', 'draftOwnership']);
});
it('an authoritatively attached destination commits synchronously and supersedes held cold settlement', async () => {
  const f = await fixture(); f.server.held.add('session/attach');
  f.server.held.add('session/checkpoint'); f.server.held.add('session/subscribe');
  const checkpointCount = f.server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  const subscriptionCount = f.server.requests.filter(row => row.request.method === 'session/subscribe').length;
  const controlCurrent = () => f.server.client.isAttachmentControlCurrent('C', f.server.client.getSnapshot().views.C.attachmentObservation);
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: true } });
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'B' });
  f.server.reply(request);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'B' }); expect(f.actor.getSnapshot().context.error).toBeUndefined();
  expect(mutations(f.server).filter(request => request.method === 'session/attach')).toHaveLength(3);
  const read = await f.server.waitFor('session/checkpoint', checkpointCount + 1);
  const target = f.server.client.target('C');
  expect(read.method).toBe('session/checkpoint');
  if (read.method !== 'session/checkpoint') throw new Error('Expected native checkpoint request');
  expect(read.params.target).toEqual(target); expect(controlCurrent()).toBe(false);
  expect(f.server.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(subscriptionCount);
  f.server.reply(read);
  const subscription = await f.server.waitFor('session/subscribe', subscriptionCount + 1);
  if (subscription.method !== 'session/subscribe') throw new Error('Expected native subscription request');
  expect(subscription.params.target).toEqual(target);
  expect(subscription.params.after_cursor).toBe(f.server.client.getSnapshot().views.C.checkpoint?.cursor);
  expect(controlCurrent()).toBe(false);
  await expect(f.server.client.send('C', 'not before the subscription receipt')).rejects.toThrow();
  f.server.reply(subscription);
  await f.server.client.waitForAttachment('C', () => true);
  expect(controlCurrent()).toBe(true);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'B' });
  expect(f.server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(checkpointCount + 1);
  expect(f.server.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(subscriptionCount + 1);
  expect(mutations(f.server).filter(request => request.method === 'session/attach')).toHaveLength(3);
  expect(mutations(f.server).filter(request => request.method !== 'session/attach')).toHaveLength(0);
});
for (const target of [userSettingsTarget, workspaceSettingsTarget('project', 'rustX')]) it(`Settings ${target.kind} preserves the selected Session owner while retiring only sidebar closure`, async () => {
  const f = await fixture(), controls = f.capture(); f.server.held.add('session/attach');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3);
  const pending = f.actor.getSnapshot().context.pending;
  f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target } });
  expect(f.settings.getSnapshot().context.target).toEqual(target); expect(controls()).toBe(false);
  expect(f.actor.getSnapshot().context.pending).toBe(pending);
  expect(f.actor.getSnapshot().context.dismissedSidebar).toBe(pending);
  f.server.reply(request); await f.server.client.waitForAttachment('C', () => true); await idle(f.actor);
  expect(f.actor.getSnapshot().context.publication?.sidebar).toBe(false);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'C' }); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(controls()).toBe(false);
  expect(f.server.client.isAttachmentControlCurrent('A', f.server.client.getSnapshot().views.A.attachmentObservation)).toBe(true);
  expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority', 'draftOwnership']);
});
it('drawer dismissal retains the exact selected target but not the right to close a reopened drawer', async () => {
  const f = await fixture(), original = f.server.client.target('A'); f.server.held.add('session/attach');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const request = await f.server.waitFor('session/attach', 3), pending = f.actor.getSnapshot().context.pending;
  f.actor.send({ type: 'SIDEBAR.DISMISS' });
  expect(f.actor.getSnapshot().context.pending).toBe(pending);
  expect(f.actor.getSnapshot().context.dismissedSidebar).toBe(pending);
  f.server.reply(request); await f.server.client.waitForAttachment('C', () => true); await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'C' });
  expect(f.actor.getSnapshot().context.publication).toMatchObject({ intent: pending, sidebar: false });
  expect(f.server.client.target('A')).toBe(original);
  expect(f.server.requests.filter(row => row.request.method === 'session/submissionIntent' || row.request.method.startsWith('turn/'))).toEqual([]);
});
it('a newer selection and explicit cancellation still retire a dismissed cold target owner', async () => {
  for (const next of ['newer', 'cancel'] as const) {
    const f = await fixture(); f.server.held.add('session/attach');
    f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
    const request = await f.server.waitFor('session/attach', 3);
    f.actor.send({ type: 'SIDEBAR.DISMISS' });
    if (next === 'newer') f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: true } });
    else f.actor.send({ type: 'CANCEL' });
    f.server.reply(request); await f.server.client.waitForAttachment('C', () => true); await idle(f.actor);
    expect(f.actor.getSnapshot().context.route).toEqual(next === 'newer' ? { kind: 'session', sessionId: 'B' } : { kind: 'session-draft', sessionId: 'C' });
    expect(f.actor.getSnapshot().context.pending).toBeUndefined();
    expect(f.server.requests.filter(row => row.request.method === 'session/submissionIntent' || row.request.method.startsWith('turn/'))).toEqual([]);
  }
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
  expect(controls()).toBe(false); expect(f.actor.getSnapshot().context.route.kind).toBe('new-conversation'); expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority', 'draftOwnership']);
});

it.each(['cancel', 'newer Session', 'new authority'] as const)('cold reconnect cannot revive a publication retired by %s', async outcome => {
  const f = await fixture();
  const reads = f.server.requests.filter(row => row.request.method === 'session/read').length;
  f.server.held.add('session/read');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const metadata = await f.server.waitFor('session/read', reads + 1);
  const intent = f.actor.getSnapshot().context.pending!;
  const oldSocket = f.server.socket, lateMetadata = f.server.commit(metadata, oldSocket);
  if (outcome === 'cancel') f.actor.send({ type: 'CANCEL', pending: intent });
  else if (outcome === 'newer Session') f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: false } });
  else f.server.authorityId = 'replacement-native-authority';
  oldSocket.close(); f.server.held.delete('session/read');
  await f.server.connect();
  if (outcome !== 'new authority') await f.server.client.waitForAttachment('C', () => true);
  const native = f.server.client.getSnapshot();
  f.actor.send({ type: 'SESSION.RESUME', intent, generation: native.generation, authority: native.authorityRevision });
  oldSocket.deliver(lateMetadata);
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual(outcome === 'cancel' ? { kind: 'session-draft', sessionId: 'C' }
    : outcome === 'newer Session' ? { kind: 'session', sessionId: 'B' } : { kind: 'new-conversation' });
  expect(f.actor.getSnapshot().context.pending).toBeUndefined();
  expect(f.actor.getSnapshot().context.draftSession).toBeUndefined();
  expect(f.server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer', 'turn/cancel', 'session/detach'].includes(row.request.method))).toEqual([]);
});

it.each(['Settings', 'newer Session', 'newer cold Session'] as const)('native draft creation keeps settlement while %s supersedes its route publication', async destination => {
  const f = await fixture(); f.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  const control = f.actor.getSnapshot().context.control, generation = f.server.client.getSnapshot().generation, binding = String(f.actor.getSnapshot().context.binding);
  const { firstSubmitPort } = await import('../src/app/new-conversation/port');
  const host = { ...f.server.workspaceHost, resolveWorkspace: async () => ({ cwd: '/workspace/A' }) };
  f.server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'A-created', active_node: 'created-node', active_conversation_id: 'created-conversation', node_count: 1, created_at: '0', updated_at: '0' } }));
  f.server.summaries.set('A-created', { cwd: '/workspace/A' }); f.server.snapshots.set('A-created', { ...snapshot('A-created'), conversation_id: 'created-conversation' }); f.server.nodeSnapshots.set('created-node', f.server.snapshots.get('A-created')!);
  f.server.held.add('session/create');
  const port = firstSubmitPort(f.server.client, host, f.capture(), () => {
    f.actor.send({ type: 'CREATE', control, binding }); const pending = f.actor.getSnapshot().context.pending;
    return sessionId => { f.actor.send({ type: 'CREATED', sessionId, control, generation, pending }); return f.capture(); };
  });
  const work = firstSubmissions(f.server.client).submit(binding, { workspaceId: 'workspace-a', text: 'Native execution intent', files: [] }, port);
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
  expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority', 'draftOwnership']);
  expect(firstSubmissions(f.server.client).draft(binding)?.session?.id).toBe('A-created');
  expect(f.server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(destination === 'Settings' ? 1 : 0);
  if (destination === 'Settings') {
    f.settings.send({ type: 'CLOSE' });
    f.actor.send({ type: 'DRAFT', workspaceId: 'workspace-b' });
    expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'new-conversation', workspaceId: 'workspace-b' });
    expect(String(f.actor.getSnapshot().context.binding)).not.toBe(binding);
  }
});

it('a native attachment ready proof publishes its route across a later same-claim resync', async () => {
  const f = await fixture();
  f.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  const attach = f.server.client.attach.bind(f.server.client);
  const intercept = vi.spyOn(f.server.client, 'attach').mockImplementation((id, node, current, attached) => attach(id, node, current, target => {
    attached?.(target);
    f.server.held.add('session/checkpoint');
    void f.server.client.refresh(id).catch(() => {});
  }));
  try {
    f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
    await f.server.waitFor('session/checkpoint', 4);
    await idle(f.actor);
    const proof = f.server.client.getSnapshot().views.C.attachmentObservation;
    expect(f.server.client.isAttachmentObservationCurrent('C', proof)).toBe(true);
    expect(f.server.client.isAttachmentControlCurrent('C', proof)).toBe(false);
    expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'C' });
    expect(mutations(f.server).filter(request => request.method === 'session/attach')).toHaveLength(3);
  } finally { intercept.mockRestore(); }
});

it('an identity receipt without checkpoint readiness cannot publish an executable route', async () => {
  const f = await fixture();
  f.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  f.server.held.add('session/checkpoint');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const checkpoint = await f.server.waitFor('session/checkpoint', 3);
  const proof = f.server.client.getSnapshot().views.C.attachmentObservation;
  expect(f.server.client.isAttachmentObservationCurrent('C', proof)).toBe(true);
  expect(f.server.client.isAttachmentControlCurrent('C', proof)).toBe(false);
  f.server.socket.deliver({ jsonrpc: '2.0', id: checkpoint.id, error: { code: -32000, message: 'Native source unavailable', data: { kind: 'operation_failed' } } });
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'C' });
  expect(f.actor.getSnapshot().context.error).toContain('Native source unavailable');
  expect(f.server.requests.filter(row => row.request.method === 'session/subscribe' && row.request.params.target.session_id === 'C')).toHaveLength(0);
});

it.each(['cancel', 'newer Session', 'new authority'] as const)('same-generation observation recovery cannot restore publication after %s', async outcome => {
  const f = await fixture();
  const count = f.server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  f.server.held.add('session/checkpoint');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const failed = await f.server.waitFor('session/checkpoint', count + 1), intent = f.actor.getSnapshot().context.pending!;
  f.server.socket.deliver({ jsonrpc: '2.0', id: failed.id, error: { code: -32000, message: 'C observation unavailable', data: { kind: 'operation_failed' } } });
  await idle(f.actor);
  const generation = f.server.client.getSnapshot().generation, authority = f.server.client.getSnapshot().authorityRevision;
  const refresh = f.server.client.refresh('C').then(() => ({ ok: true as const }), error => ({ ok: false as const, error }));
  const checkpoint = await f.server.waitFor('session/checkpoint', count + 2);
  if (outcome === 'cancel') f.actor.send({ type: 'CANCEL' });
  else if (outcome === 'newer Session') f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: true } });
  else {
    f.server.authorityId = 'replacement-native-authority';
    f.server.socket.close(); await f.server.connect();
  }
  f.server.reply(checkpoint); const result = await refresh; await idle(f.actor);
  if (outcome === 'new authority') {
    expect(result).toMatchObject({ ok: false, error: expect.any(Error) });
    expect(f.server.client.getSnapshot().authorityRevision).not.toBe(authority);
    expect(f.server.client.isAttachmentControlCurrent('C', f.server.client.getSnapshot().views.C?.attachmentObservation)).toBe(false);
  } else {
    expect(result).toEqual({ ok: true });
    expect(f.server.client.getSnapshot().generation).toBe(generation);
    expect(f.server.client.isAttachmentControlCurrent('C', f.server.client.getSnapshot().views.C.attachmentObservation)).toBe(true);
  }
  const native = f.server.client.getSnapshot();
  f.actor.send({ type: 'SESSION.RESUME', intent, generation: native.generation, authority: native.authorityRevision });
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual(outcome === 'cancel' ? { kind: 'session-draft', sessionId: 'C' } : outcome === 'newer Session' ? { kind: 'session', sessionId: 'B' } : { kind: 'new-conversation' });
  expect(f.actor.getSnapshot().context.draftSession).toBeUndefined();
  expect(f.server.requests.filter(row => row.request.method === 'session/attach' && row.request.params.session_id === 'C')).toHaveLength(1);
  expect(f.server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer', 'turn/cancel', 'session/detach'].includes(row.request.method))).toEqual([]);
});
