import { afterEach, expect, it } from 'vitest';
import { waitFor } from 'xstate';
import { Server, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import type { ForkOrigin } from '../src/app/agent/ForkPoint';
const dispose: (() => void)[] = [];
afterEach(() => dispose.splice(0).forEach(stop => stop()));
async function fixture(source = 'C', authorized = true) {
  const server = new Server();
  server.snapshots.get('A')!.attempt = { attempt_id: 'active-A', phase: { type: 'running' }, turn: 1 };
  server.snapshots.get('B')!.attempt = { attempt_id: 'active-B', phase: { type: 'running' }, turn: 1 };
  await server.attached('A', 'B'); server.snapshots.set('C', snapshot('C'));
  server.nodeSnapshots.set('historical-node', { ...snapshot(source), conversation_id: 'immutable-source', transcript: { entries: Array.from({ length: 200 }, (_, i) => ({ cursor: String(i + 1), item: { type: 'message' as const, message: { id: `source-${i}`, role: 'user' as const, source: 'human' as const, kind: 'message' as const, content: [{ type: 'text' as const, text: `Historical message ${i}` }] } } })) } });
  const f = routeFixture(server.client, { ...server.workspaceHost, classifyLocations: async cwds => cwds.map(() => authorized ? { authorized: true } : { authorized: false, reason: 'denied' }) }); dispose.push(() => { f.stop(); server.client.disconnect(); });
  const origin: ForkOrigin = { type: 'fork', source_session: source, source_node: 'historical-node', source_message: 'source-150', source_surface_revision: '1', side: 'after' };
  const before = server.requests.length, target = server.client.target('A'), claims = server.claims();
  return { ...f, server, origin, before, target, claims };
}
const forbidden = (f: Awaited<ReturnType<typeof fixture>>) => f.server.requests.slice(f.before).filter(row => ['session/attach', 'session/detach', 'session/switchNode', 'turn/start', 'turn/steer'].includes(row.request.method));
for (const source of ['A', 'B', 'C']) it(`View Source reads exact non-selected Node and distant Message without executable effects: ${source}`, async () => {
  const f = await fixture(source);
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } });
  await waitFor(f.actor, snapshot => snapshot.matches('idle'));
  const route = f.actor.getSnapshot().context.route;
  expect(route).toMatchObject({ kind: 'history', sessionId: source, nodeId: 'historical-node', messageId: 'source-150', conversationId: 'immutable-source' });
  if (route.kind !== 'history') throw new Error(f.actor.getSnapshot().context.error);
  expect(route.window.target_cursor).toBe('151'); expect(route.window.page.entries).toHaveLength(64);
  expect(f.server.requests.slice(f.before).filter(row => row.request.method === 'session/history').map(row => row.request.params)).toEqual([{ session_id: source, node_id: 'historical-node', at: { type: 'message', id: 'source-150' }, limit: 64 }]);
  expect(forbidden(f)).toEqual([]); expect(f.server.claims()).toEqual(f.claims); expect(f.server.client.target('A')).toBe(f.target);
  f.actor.send({ type: 'RETURN', route }); expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' }); expect(f.server.client.target('A')).toBe(f.target);
});
for (const missing of ['node', 'message']) it(`missing source ${missing} preserves committed controls and attachment`, async () => {
  const f = await fixture(), controls = f.capture(); f.origin[missing === 'node' ? 'source_node' : 'source_message'] = 'absent';
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } }); await waitFor(f.actor, snapshot => snapshot.matches('idle'));
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' }); expect(f.actor.getSnapshot().context.error).toContain(missing === 'node' ? 'Unknown Node' : 'Unknown Message');
  expect(controls()).toBe(true); expect(f.server.client.target('A')).toBe(f.target); expect(forbidden(f)).toEqual([]);
});
for (const method of ['session/node', 'session/history'] as const) for (const target of [userSettingsTarget, workspaceSettingsTarget('project', 'rustX')]) for (const successful of [true, false]) it(`Settings ${target.kind} supersedes ${successful ? 'successful' : 'failed'} ${method} without stale publication`, async () => {
  const f = await fixture(), controls = f.capture(); f.server.held.add(method);
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } });
  const request = await f.server.waitFor(method, f.server.requests.slice(0, f.before).filter(row => row.request.method === method).length + 1);
  f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target } });
  if (successful) f.server.reply(request); else f.server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'obsolete failure', data: { kind: 'operation_failed' } } });
  await f.server.client.request({ method: 'server/info', params: {} }, 'server_info');
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' }); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(controls()).toBe(true);
  expect(f.settings.getSnapshot().context.target).toEqual(target); expect(f.settings.getSnapshot().context.page).toBeDefined(); expect(forbidden(f)).toEqual([]);
  if (method === 'session/node') expect(f.server.requests.slice(f.before).some(row => row.request.method === 'session/history')).toBe(false);
});
it('a newer route and a replaced connection retire held history without stale errors or native mutation', async () => {
  for (const replacement of ['route', 'connection']) {
    const f = await fixture(); f.server.held.add('session/history');
    f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } }); const request = await f.server.waitFor('session/history', f.server.requests.slice(0, f.before).filter(row => row.request.method === 'session/history').length + 1);
    const response = f.server.commit(request);
    if (replacement === 'route') f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: false } });
    else { f.server.client.disconnect(); f.actor.send({ type: 'AUTHORITY', generation: f.server.client.getSnapshot().generation, authority: f.server.client.getSnapshot().authorityRevision }); }
    f.server.socket.deliver(response); await Promise.resolve();
    expect(f.actor.getSnapshot().context.route.kind).not.toBe('history'); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority']); expect(forbidden(f)).toEqual([]);
  }
});
it.each([true, false])('newer history intent wins when the older read arrives last: old success=%s', async successful => {
  const f = await fixture(); f.server.held.add('session/history');
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } }); const older = await f.server.waitFor('session/history', f.server.requests.slice(0, f.before).filter(row => row.request.method === 'session/history').length + 1);
  f.server.nodeSnapshots.set('newer-history-node', { ...snapshot('B'), conversation_id: 'newer-history-conversation', transcript: { entries: [{ cursor: '900', item: { type: 'message', message: { id: 'newer-message', role: 'user', source: 'human', content: [{ type: 'text', text: 'Newer source' }] } } }] } });
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: { ...f.origin, source_session: 'B', source_node: 'newer-history-node', source_message: 'newer-message' } } });
  const newer = await Promise.race([f.server.waitFor('session/history', f.server.requests.slice(0, f.before).filter(row => row.request.method === 'session/history').length + 2), waitFor(f.actor, snapshot => snapshot.matches('idle')).then(() => { throw new Error(f.actor.getSnapshot().context.error ?? 'Preparation stopped before history dispatch'); })]); f.server.reply(newer); await waitFor(f.actor, snapshot => snapshot.matches('idle'));
 const route = f.actor.getSnapshot().context.route; expect(route).toMatchObject({ kind: 'history', sessionId: 'B', nodeId: 'newer-history-node', messageId: 'newer-message' });
  if (successful) f.server.reply(older); else f.server.socket.deliver({ jsonrpc: '2.0', id: older.id, error: { code: -32000, message: 'Obsolete source failure', data: { kind: 'operation_failed' } } });
 await f.server.client.request({ method: 'server/info', params: {} }, 'server_info');
  expect(f.actor.getSnapshot().context.route).toBe(route); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(forbidden(f)).toEqual([]); expect(Object.keys(f.actor.getSnapshot().children)).toEqual(['nativeAuthority']);
});
it('history reading fails closed on exact Product Host cwd authorization without acquiring execution authority', async () => {
  const f = await fixture('C', false);
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } }); await waitFor(f.actor, snapshot => snapshot.matches('idle'));
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' }); expect(f.actor.getSnapshot().context.error).toContain('Session cwd is not authorized');
  expect(f.server.requests.slice(f.before).some(row => row.request.method === 'session/history')).toBe(false); expect(forbidden(f)).toEqual([]);
});
for (const method of ['session/node', 'session/history'] as const) for (const success of [true, false]) it(`replaced read admission retires ${method} ${success ? 'success' : 'failure'} without publication or execution`, async () => {
  const f = await fixture(), controls = f.capture(); f.server.held.add(method);
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } });
  const request = await f.server.waitFor(method, f.server.requests.slice(0, f.before).filter(row => row.request.method === method).length + 1);
  const proof = f.actor.getSnapshot().context.readAdmission; expect(proof?.current()).toBe(true);
  f.server.client.setSessionAdmission(async () => false); expect(proof?.current()).toBe(false);
  if (success) f.server.reply(request); else f.server.socket.deliver({jsonrpc:'2.0',id:request.id,error:{code:-32000,message:'Obsolete read failure',data:{kind:'operation_failed'}}});
  await waitFor(f.actor, snapshot => snapshot.matches('idle'));
  expect(f.actor.getSnapshot().context.route).toEqual({kind:'session',sessionId:'A'}); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(controls()).toBe(true); expect(forbidden(f)).toEqual([]);
  if(method==='session/node')expect(f.server.requests.slice(f.before).some(row=>row.request.method==='session/history')).toBe(false);
});

it.each(['history', 'session'] as const)('an obsolete viewport cancellation cannot retire a newer %s intent', async newer => {
  const f = await fixture(); f.server.held.add('session/history');
  f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } });
  const older = await f.server.waitFor('session/history', f.server.requests.slice(0, f.before).filter(row => row.request.method === 'session/history').length + 1);
  const pending = f.actor.getSnapshot().context.pending!;
  if (newer === 'history') f.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: { ...f.origin, source_message: 'source-151' } } });
  else f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: false } });
  const next = f.actor.getSnapshot().context.pending, control = f.capture();
  f.actor.send({ type: 'CANCEL', pending });
  expect(f.actor.getSnapshot().context.pending).toBe(next); expect(control()).toBe(true);
  if (newer === 'history') {
    const request = await f.server.waitFor('session/history', f.server.requests.slice(0, f.before).filter(row => row.request.method === 'session/history').length + 2);
    f.server.reply(request); await waitFor(f.actor, state => state.matches('idle'));
  }
  const route = f.actor.getSnapshot().context.route;
  expect(route).toMatchObject(newer === 'history' ? { kind: 'history', messageId: 'source-151' } : { kind: 'session', sessionId: 'B' });
  f.actor.send({ type: 'CANCEL', pending }); f.server.reply(older);
  await f.server.client.request({ method: 'server/info', params: {} }, 'server_info');
  expect(f.actor.getSnapshot().context.route).toBe(route); expect(f.actor.getSnapshot().context.error).toBeUndefined(); expect(forbidden(f)).toEqual([]);
});
