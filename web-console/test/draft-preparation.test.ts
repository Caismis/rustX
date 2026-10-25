import { afterEach, expect, it, vi } from 'vitest';
import { firstSubmissions, type FirstSubmitPort } from '../src/app/new-conversation/first-submit';
import { OutcomeUncertain, RequestNotDispatched } from '../src/client/errors';
import { DraftPreparations } from '../src/app/new-conversation/preparation';
import { firstSubmitPort } from '../src/app/new-conversation/port';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { Server, endpoint, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';
import { userSettingsTarget } from '../src/app/settings/projection';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../src/workspaces/host';
const disposals: (() => void)[] = [];
afterEach(() => disposals.splice(0).forEach(dispose => dispose()));
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
async function fixture(overrides: Partial<ProductHostWorkspaces> = {}) {
  const server = new Server(); await server.connect();
  const host: ProductHostWorkspaces = { ...server.workspaceHost, resolveWorkspace: async id => ({ cwd: id === 'workspace-a' ? '/workspace/A' : '/workspace/B' }), ...overrides };
  const authority = new WorkspaceAuthority(host);
  const owner = new DraftPreparations(server.client, host, authority);
  disposals.push(() => { owner.dispose(); server.client.disconnect(); });
  return { server, host, authority, owner, requests: () => server.requests.map(row => row.request) };
}
it('passive Workspace observation starts no preparation or Session; selected authorized draft starts MCP only', async () => {
  const f = await fixture(); await f.authority.observe();
  expect(f.requests().filter(row => row.method.startsWith('mcp/') || row.method === 'session/create')).toEqual([]);
  f.owner.follow('draft');
  await Promise.resolve(); expect(f.requests().some(row => row.method === 'mcp/draftPrepare')).toBe(false);
  f.owner.follow('draft', 'workspace-a');
  const request = await f.server.waitFor('mcp/draftPrepare', 1);
  expect(request.params).toMatchObject({ cwd: '/workspace/A', target: { host_authority: 'fixture-host' } });
  expect(f.requests().some(row => ['session/create', 'turn/start', 'session/uploadPrepare'].includes(row.method))).toBe(false);
});
it('repeated navigation observations and same-Host remount retain exactly one preparation', async () => {
  const f = await fixture(); f.owner.follow('draft', 'workspace-a');
  const request = await f.server.waitFor('mcp/draftPrepare', 1);
  for (let i = 0; i < 5; i++) f.owner.follow('draft', 'workspace-a');
  const replacement = new WorkspaceAuthority(f.host);
  f.owner.configure(f.host, replacement); await replacement.observe();
  f.owner.follow('draft', 'workspace-a'); await Promise.resolve();
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toEqual([request]);
  expect(f.requests().filter(row => row.method === 'mcp/draftCancel')).toEqual([]);
});
it('A to B to A correlates monotonic generations and ignores an obsolete authorization result', async () => {
  const first = deferred<{ cwd: string }>(), entered = deferred<void>(); let calls = 0;
  const f = await fixture({ resolveWorkspace: async id => { if (++calls === 1) { entered.resolve(); return first.promise; } return { cwd: id === 'workspace-a' ? '/workspace/A' : '/workspace/B' }; } });
  f.owner.follow('draft', 'workspace-a'); await entered.promise;
  f.owner.follow('draft', 'workspace-b'); const b = await f.server.waitFor('mcp/draftPrepare', 1);
  f.owner.follow('draft', 'workspace-a'); const a = await f.server.waitFor('mcp/draftPrepare', 2);
  first.resolve({ cwd: '/workspace/A' }); await Promise.resolve(); await Promise.resolve();
  if (a.method !== 'mcp/draftPrepare' || b.method !== 'mcp/draftPrepare') throw Error('wrong method');
  expect(a.params.target.generation).toBeGreaterThan(b.params.target.generation);
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toEqual([b, a]);
  expect(f.requests().some(row => row.method === 'session/create')).toBe(false);
});
it('Send seals an exact target; navigation after transfer cannot cancel it or retarget its continuation', async () => {
  const f = await fixture(); f.owner.follow('draft', 'workspace-a');
  const request = await f.server.waitFor('mcp/draftPrepare', 1);
  if (request.method !== 'mcp/draftPrepare') throw Error('wrong method');
  expect(f.owner.seal('draft', 'workspace-a', 'fixture-host')).toEqual(request.params.target);
  f.owner.follow('draft', 'workspace-b'); await Promise.resolve();
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toHaveLength(1);
  f.owner.follow(); f.owner.follow('next-draft', 'workspace-b');
  await f.server.waitFor('mcp/draftPrepare', 2);
  expect(f.requests().filter(row => row.method === 'mcp/draftCancel').map(row => row.params)).toEqual([{ target: request.params.target }]);
});
it.each(['attachment', 'Settings', 'same Session', 'abandonment', 'discarded failure'] as const)('creation ACK preserves the submitted generation across %s, before native transfer can finish', async outcome => {
  const f = await fixture(), navigation = routeFixture(f.server.client, f.host, f.owner);
  navigation.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  const prepared = await f.server.waitFor('mcp/draftPrepare', 1);
  if (prepared.method !== 'mcp/draftPrepare') throw Error('wrong method');
  const { binding, control } = navigation.actor.getSnapshot().context, generation = f.server.client.getSnapshot().generation;
  const session = { id: 'created', active_node: 'created-node', active_conversation_id: 'created-conversation', node_count: 1, created_at: '0', updated_at: '0' };
  f.server.handlers.set('session/create', () => ({ type: 'session_transition', session }));
  f.server.summaries.set(session.id, { cwd: '/workspace/A' });
  f.server.snapshots.set(session.id, { ...snapshot(session.id), conversation_id: session.active_conversation_id });
  f.server.nodeSnapshots.set(session.active_node, f.server.snapshots.get(session.id)!);
  f.server.held.add('session/create'); f.server.held.add('session/attach');
  const port = firstSubmitPort(f.server.client, f.host, navigation.capture(), () => {
    navigation.actor.send({ type: 'CREATE', control, binding: String(binding) });
    const pending = navigation.actor.getSnapshot().context.pending;
    return sessionId => { navigation.actor.send({ type: 'CREATED', sessionId, control, generation, pending }); return navigation.capture(); };
  });
  const preparation = f.owner.seal(String(binding), 'workspace-a', 'fixture-host');
  const work = firstSubmissions(f.server.client).submit(String(binding), { workspaceId: 'workspace-a', text: 'One retained initialization', files: [], preparation }, port);
  const create = await f.server.waitFor('session/create', 1);
  // Deliver only the ordinary ACK. Native composition/transfer is represented
  // by the held attachment; no rename or extra native work precedes this ACK.
  f.server.reply(create);
  const attach = await f.server.waitFor('session/attach', 1);
  expect(navigation.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: session.id });
  expect(firstSubmissions(f.server.client).draft(String(binding))?.phase).toBe('attaching');
  expect(f.requests().filter(row => row.method === 'mcp/draftCancel')).toEqual([]);
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toEqual([prepared]);
  if (outcome === 'Settings') navigation.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target: userSettingsTarget } });
  else if (outcome === 'same Session') navigation.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: session.id, sidebar: true } });
  if (outcome === 'abandonment') {
    navigation.actor.send({ type: 'WORKFLOWS' });
    const cancel = await f.server.waitFor('mcp/draftCancel', 1);
    expect(cancel.params).toEqual({ target: preparation });
  } else expect(f.requests().filter(row => row.method === 'mcp/draftCancel')).toEqual([]);
  if (outcome === 'discarded failure') {
    f.server.socket.deliver({ jsonrpc: '2.0', id: attach.id, error: { code: -32000, message: 'initialization failed', data: { kind: 'operation_failed' } } });
    expect(await work).toBe(false);
    const submissions = firstSubmissions(f.server.client), flow = submissions.draft(String(binding))!;
    expect(flow).toMatchObject({ phase: 'failed', session: { id: session.id } });
    const observations = vi.spyOn(f.host, 'listWorkspaces');
    expect(submissions.discard(flow)).toBe(true);
    await f.server.waitFor('mcp/draftCancel', 1);
    expect(observations).not.toHaveBeenCalled();
    expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toEqual([prepared]);
    expect(f.requests().filter(row => row.method === 'session/create')).toHaveLength(1);
    expect(f.requests().filter(row => row.method === 'turn/start')).toHaveLength(0);
    navigation.actor.send({ type: 'DRAFT', workspaceId: 'workspace-b' });
    await f.server.waitFor('mcp/draftPrepare', 2);
    return;
  }
  f.server.reply(attach);
  expect(await work).toBe(outcome !== 'abandonment');
  expect(navigation.actor.getSnapshot().context.route).toEqual(outcome === 'abandonment' ? { kind: 'workflows' }
    : { kind: outcome === 'Settings' ? 'session-draft' : 'session', sessionId: session.id });
  expect(f.requests().filter(row => row.method === 'session/create')).toHaveLength(1);
  expect(f.requests().filter(row => row.method === 'turn/start')).toHaveLength(outcome !== 'abandonment' ? 1 : 0);
});
it.each([
  { outcome: 'admitted', next: 'Workspace selection' }, { outcome: 'admitted', next: 'New Conversation' },
  { outcome: 'discarded failure', next: 'Workspace selection' }, { outcome: 'discarded failure', next: 'Workspace picker' },
  { outcome: 'discarded failure', next: 'New Conversation' },
] as const)('Settings before creation ACK permits $next after $outcome with a fresh binding and preparation', async ({ outcome, next }) => {
  const f = await fixture(), navigation = routeFixture(f.server.client, f.host, f.owner);
  navigation.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  const prepared = await f.server.waitFor('mcp/draftPrepare', 1);
  if (prepared.method !== 'mcp/draftPrepare') throw Error('wrong method');
  const { binding, control } = navigation.actor.getSnapshot().context, generation = f.server.client.getSnapshot().generation;
  const session = { id: 'created', active_node: 'created-node', active_conversation_id: 'created-conversation', node_count: 1, created_at: '0', updated_at: '0' };
  f.server.handlers.set('session/create', () => ({ type: 'session_transition', session }));
  f.server.summaries.set(session.id, { cwd: '/workspace/A' });
  f.server.snapshots.set(session.id, { ...snapshot(session.id), conversation_id: session.active_conversation_id });
  f.server.nodeSnapshots.set(session.active_node, f.server.snapshots.get(session.id)!);
  f.server.held.add('session/create'); f.server.held.add('session/attach');
  const port = firstSubmitPort(f.server.client, f.host, navigation.capture(), () => {
    navigation.actor.send({ type: 'CREATE', control, binding: String(binding) });
    const pending = navigation.actor.getSnapshot().context.pending;
    return sessionId => { navigation.actor.send({ type: 'CREATED', sessionId, control, generation, pending }); return navigation.capture(); };
  });
  const preparation = f.owner.seal(String(binding), 'workspace-a', 'fixture-host');
  const submissions = firstSubmissions(f.server.client);
  const work = submissions.submit(String(binding), { workspaceId: 'workspace-a', text: 'One admitted message', files: [], preparation }, port);
  const create = await f.server.waitFor('session/create', 1);
  navigation.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target: userSettingsTarget } });
  navigation.actor.send({ type: 'DRAFT', workspaceId: 'workspace-b' });
  expect(navigation.actor.getSnapshot().context).toMatchObject({ binding, route: { kind: 'new-conversation', workspaceId: 'workspace-a' } });
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toEqual([prepared]);
  f.server.reply(create);
  const attach = await f.server.waitFor('session/attach', 1);
  expect(navigation.actor.getSnapshot().context.route).toEqual({ kind: 'new-conversation', workspaceId: 'workspace-a' });
  if (outcome === 'discarded failure') {
    f.server.socket.deliver({ jsonrpc: '2.0', id: attach.id, error: { code: -32000, message: 'initialization failed', data: { kind: 'operation_failed' } } });
    expect(await work).toBe(false);
    const failed = submissions.draft(String(binding))!;
    expect(failed).toMatchObject({ phase: 'failed', session: { id: session.id } });
    expect(submissions.discard(failed)).toBe(true);
    expect(submissions.draft(String(binding))?.phase).toBe('discarded');
  } else {
    f.server.reply(attach);
    expect(await work).toBe(true);
    expect(submissions.draft(String(binding))?.phase).toBe('admitted');
  }
  navigation.settings.send({ type: 'CLOSE' });
  if (next === 'Workspace picker') navigation.actor.send({ type: 'DRAFT.WORKSPACE', workspaceId: 'workspace-b' });
  else navigation.actor.send({ type: 'DRAFT', workspaceId: next === 'Workspace selection' ? 'workspace-b' : undefined });
  const fresh = navigation.actor.getSnapshot().context.binding;
  expect(fresh).not.toBe(binding);
  if (next === 'New Conversation') navigation.actor.send({ type: 'DRAFT.WORKSPACE', workspaceId: 'workspace-b' });
  const replacement = await f.server.waitFor('mcp/draftPrepare', 2);
  expect(navigation.actor.getSnapshot().context).toMatchObject({ binding: fresh, route: { kind: 'new-conversation', workspaceId: 'workspace-b' } });
  expect(replacement.params).toMatchObject({ cwd: '/workspace/B', target: { generation: expect.any(Number) } });
  if (replacement.method !== 'mcp/draftPrepare') throw Error('wrong method');
  expect(replacement.params.target.generation).toBeGreaterThan(prepared.params.target.generation);
  expect(submissions.draft(String(fresh))).toBeUndefined();
  expect(f.requests().filter(row => row.method === 'session/create')).toHaveLength(1);
  expect(f.requests().filter(row => row.method === 'turn/start')).toHaveLength(outcome === 'admitted' ? 1 : 0);
});
it('leaving an unsent draft revokes only that generation and a different tab is independent', async () => {
  const f = await fixture(); const other = await fixture();
  f.owner.follow('draft', 'workspace-a'); other.owner.follow('draft', 'workspace-a');
  const first = await f.server.waitFor('mcp/draftPrepare', 1); const second = await other.server.waitFor('mcp/draftPrepare', 1);
  if (first.method !== 'mcp/draftPrepare' || second.method !== 'mcp/draftPrepare') throw Error('wrong method');
  expect(first.params.target.draft_id).not.toBe(second.params.target.draft_id);
  f.owner.follow(); const cancel = await f.server.waitFor('mcp/draftCancel', 1);
  expect(cancel.params).toEqual({ target: first.params.target });
  expect(other.requests().some(row => row.method === 'mcp/draftCancel')).toBe(false);
});
it('returning to an older submitted Session after a newer Send starts no draft preparation', async () => {
  const f = await fixture(), navigation = routeFixture(f.server.client, f.host, f.owner);
  const submissions = firstSubmissions(f.server.client);
  for (const [index, workspaceId] of ['workspace-a', 'workspace-b'].entries()) {
    navigation.actor.send({ type: 'DRAFT', workspaceId });
    await f.server.waitFor('mcp/draftPrepare', index + 1);
    const binding = String(navigation.actor.getSnapshot().context.binding);
    const preparation = f.owner.seal(binding, workspaceId, 'fixture-host');
    const session = { id: index ? 'B' : 'A', conversation: `conversation-${index ? 'B' : 'A'}` };
    const port: FirstSubmitPort = { current: () => true, create: async () => session,
      handoff: vi.fn(), attach: vi.fn(), upload: vi.fn(), send: vi.fn() };
    expect(await submissions.submit(binding, { workspaceId, text: 'Submitted input', files: [], preparation }, port)).toBe(true);
    navigation.actor.send({ type: 'WORKFLOWS' });
  }
  f.server.held.add('session/attach');
  const observations = vi.spyOn(f.authority, 'observe');
  navigation.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'A', sidebar: false } });
  const attach = await f.server.waitFor('session/attach', 1);
  expect(navigation.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'A' });
  expect(observations).not.toHaveBeenCalled();
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toHaveLength(2);
  f.server.reply(attach);
  await f.server.client.waitForAttachment('A', () => true);
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toHaveLength(2);
});
it('Host replacement invalidates only unsubmitted preparation and uses fresh authorization', async () => {
  let hostId = 'host-A';
  const f = await fixture({ listWorkspaces: async () => ({ authorityId: hostId, endpoint, workspaces: [{ id: 'workspace-a', displayName: 'A', location: '/workspace/A', displayPath: '/workspace/A' }], picker: { kind: 'unavailable', reason: 'fixture' } } satisfies WorkspaceCatalog) });
  f.owner.follow('draft', 'workspace-a'); await f.server.waitFor('mcp/draftPrepare', 1);
  hostId = 'host-B'; await f.authority.observe();
  const replacement = await f.server.waitFor('mcp/draftPrepare', 2);
  expect(replacement.params).toMatchObject({ target: { host_authority: 'host-B' } });
  expect(f.requests().filter(row => row.method === 'mcp/draftCancel')).toHaveLength(1);
});
it('final Host denial prevents process effects and does not create visible uncertainty', async () => {
  const denied = deferred<void>();
  const f = await fixture({ classifyLocations: async () => { denied.resolve(); return [{ authorized: false, reason: 'denied' }]; } });
  f.owner.follow('draft', 'workspace-a'); await denied.promise;
  await Promise.resolve(); await Promise.resolve();
  expect(f.requests().some(row => row.method === 'mcp/draftPrepare')).toBe(false);
  expect(f.server.client.getSnapshot().uncertain).toEqual([]);
});
it('a disconnected draft starts fresh native ownership after reconnect, without replaying Session creation', async () => {
  const f = await fixture(); f.owner.follow('draft', 'workspace-a'); await f.server.waitFor('mcp/draftPrepare', 1);
  f.server.client.disconnect(); await f.server.connect();
  await f.server.waitFor('mcp/draftPrepare', 2);
  expect(f.requests().some(row => row.method === 'session/create')).toBe(false);
});

it('an unresolved first Send never prepares a second draft after transport reconnect', async () => {
  const f = await fixture(); f.owner.follow('draft', 'workspace-a');
  await f.server.waitFor('mcp/draftPrepare', 1);
  f.owner.seal('draft', 'workspace-a', 'fixture-host');
  const lists = vi.spyOn(f.host, 'listWorkspaces');
  f.server.client.disconnect(); await f.server.connect();
  // Native connection publication calls the owner synchronously. A new
  // preparation would start its Host observation before its first await.
  expect(lists).not.toHaveBeenCalled();
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toHaveLength(1);
  f.owner.follow('fresh-draft', 'workspace-b');
  await f.server.waitFor('mcp/draftPrepare', 2);
});

it.each(['rejected', 'discarded'] as const)('a %s first Send releases the editable binding for fresh Workspace preparation', async outcome => {
  const f = await fixture(); f.owner.follow('draft', 'workspace-a');
  const initial = await f.server.waitFor('mcp/draftPrepare', 1);
  const preparation = f.owner.seal('draft', 'workspace-a', 'fixture-host');
  const submissions = firstSubmissions(f.server.client);
  const port: FirstSubmitPort = { current: () => true,
    create: async () => { throw outcome === 'rejected' ? new RequestNotDispatched('Host admission denied') : new OutcomeUncertain(); },
    handoff: vi.fn(), attach: vi.fn(), upload: vi.fn(), send: vi.fn() };
  expect(await submissions.submit('draft', { workspaceId: 'workspace-a', text: 'Retained draft', files: [], preparation }, port)).toBe(false);
  if (outcome === 'discarded') {
    expect(submissions.draft('draft')?.phase).toBe('uncertain');
    f.server.client.disconnect(); await f.server.connect();
    expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toHaveLength(1);
    expect(submissions.discard(submissions.draft('draft')!)).toBe(true);
  }
  f.owner.follow('draft', 'workspace-b');
  const replacement = await f.server.waitFor('mcp/draftPrepare', 2);
  expect(replacement.params).toMatchObject({ cwd: '/workspace/B' });
  if (initial.method !== 'mcp/draftPrepare' || replacement.method !== 'mcp/draftPrepare') throw Error('wrong method');
  expect(replacement.params.target.generation).toBeGreaterThan(initial.params.target.generation);
  expect(port.handoff).not.toHaveBeenCalled(); expect(port.attach).not.toHaveBeenCalled(); expect(port.send).not.toHaveBeenCalled();
});

it.each(['rejected', 'discarded'] as const)('an older %s submission cannot retire a different current draft', async outcome => {
  const f = await fixture(); f.owner.follow('A', 'workspace-a');
  const a = await f.server.waitFor('mcp/draftPrepare', 1);
  const preparation = f.owner.seal('A', 'workspace-a', 'fixture-host');
  const settled = deferred<void>(), submissions = firstSubmissions(f.server.client);
  const port: FirstSubmitPort = { current: () => true,
    create: async () => { await settled.promise; throw outcome === 'rejected' ? new RequestNotDispatched('A denied') : new OutcomeUncertain(); },
    handoff: vi.fn(), attach: vi.fn(), upload: vi.fn(), send: vi.fn() };
  const work = submissions.submit('A', { workspaceId: 'workspace-a', text: 'Older Send A', files: [], preparation }, port);
  f.owner.follow(); f.owner.follow('B', 'workspace-b');
  const b = await f.server.waitFor('mcp/draftPrepare', 2);
  const observations = vi.spyOn(f.host, 'listWorkspaces');
  settled.resolve(); expect(await work).toBe(false);
  if (outcome === 'discarded') expect(submissions.discard(submissions.draft('A')!)).toBe(true);
  expect(observations).not.toHaveBeenCalled();
  expect(f.requests().filter(row => row.method === 'mcp/draftPrepare')).toEqual([a, b]);
  expect(f.requests().filter(row => row.method === 'mcp/draftCancel').map(row => row.params)).toEqual([{ target: preparation }]);
});
