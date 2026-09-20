import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v14';
import { interactionKey, OutcomeUncertain } from '../src/client/app-server';
import { conversation } from '../src/bindings/projection';
import { capabilities, endpoint, interaction, Server, snapshot, TOKEN } from './fixture';
const servers: Server[] = [];
const server = () => { const value = new Server(); servers.push(value); return value; };
afterEach(() => { for (const s of servers.splice(0)) s.client.disconnect(); vi.useRealTimers(); });

describe('native App Server connection', () => {
  it('initializes with browser admission and generated protocol capabilities', async () => {
    const s = server(); await s.connect();
    expect(s.client.getSnapshot().connection).toBe('connected');
    expect(s.client.getSnapshot().capabilities).toEqual(capabilities);
    expect(s.requests[0].request).toMatchObject({ method: 'initialize', params: { protocol_version: 14 } });
    expect(JSON.stringify(s.client.log.getSnapshot())).not.toContain(TOKEN);
  });
  it('rejects incompatible versions and missing native capabilities', async () => {
    const s = server(); s.version = 6;
    await expect(s.connect()).rejects.toThrow('Incompatible');
    expect(s.client.getSnapshot().connection).toBe('incompatible');
    expect(s.requests.map(item => item.request.method)).toEqual(['initialize']);
    const t = server(); t.capabilities = { ...capabilities, headless_interactions: false };
    await expect(t.connect()).rejects.toThrow('Incompatible');
  });
  it('correlates pipelined responses in reverse order', async () => {
    const s = server(); await s.connect(); s.held.add('settings/read');
    const a = s.client.request({ method: 'settings/read', params: { session_id: 'A' } }, 'settings');
    const b = s.client.request({ method: 'settings/read', params: { session_id: 'B' } }, 'settings');
    const requests = s.requests.filter(item => item.request.method === 'settings/read');
    s.reply(requests[1].request); s.reply(requests[0].request);
    expect((await a).settings.cwd).toBe('/workspace/A'); expect((await b).settings.cwd).toBe('/workspace/B');
  });
  it('routes A/B notifications independently and replaces streaming with canonical messages once', async () => {
    const s = server(); await s.attached('A', 'B');
    const originalB = s.client.getSnapshot().views.B.snapshot;
    const streaming: RuntimeClientSnapshot = { ...snapshot(), attempt: { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1,
      in_flight: { message_id: 'answer', blocks: [{ type: 'text', block_index: 0, text: 'Hello' }] } } };
    await s.update('A', streaming);
    expect(s.client.getSnapshot().views.B.snapshot).toBe(originalB);
    expect(conversation(s.client.getSnapshot().views.A.snapshot!).streaming?.message_id).toBe('answer');
    await s.update('A', { ...streaming, messages: [{ id: 'answer', role: 'assistant', content: [{ type: 'text', text: 'Hello world' }] }],
      attempt: { ...streaming.attempt!, phase: { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } }, in_flight: null } });
    expect(conversation(s.client.getSnapshot().views.A.snapshot!)).toMatchObject({ messages: [{ id: 'answer' }], streaming: undefined });
    expect(s.client.getSnapshot().views.B.snapshot).toBe(originalB);
  });
  it('disconnect leaves runtime facts stale without cancellation and manual disconnect stays disconnected', async () => {
    const s = server(); await s.attached('A');
    await s.update('A', { ...snapshot(), attempt: { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1 } });
    const requests = s.requests.length; const before = s.client.getSnapshot().views.A.snapshot;
    vi.useFakeTimers(); s.client.disconnect(); await vi.advanceTimersByTimeAsync(120_000);
    expect(s.client.getSnapshot().connection).toBe('disconnected');
    expect(s.client.getSnapshot().views.A.snapshot).toBe(before);
    expect(s.client.getSnapshot().views.A.attachment).toBe('stale'); expect(s.requests).toHaveLength(requests);
  });
  it('reconnect replaces stale projections and fences obsolete connection callbacks and responses', async () => {
    const s = server(); await s.attached('A');
    const old = s.socket; const oldTarget = s.client.target('A');
    s.held.add('session/snapshot'); const staleRead = s.client.refresh('A').catch(() => {});
    const oldRequest = s.requests.at(-1)!.request;
    old.close(); s.held.delete('session/snapshot');
    s.snapshots.set('A', { ...snapshot(), messages: [{ id: 'fresh', role: 'assistant', content: [{ type: 'text', text: 'Recovered' }] }] });
    await s.connect(); await staleRead;
    const current = s.client.getSnapshot().views.A;
    old.success(oldRequest, { type: 'snapshot', snapshot: snapshot(), cursor: '999' });
    old.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: oldTarget } });
    // Even on the current socket an obsolete attachment/incarnation cannot route.
    s.socket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: oldTarget } });
    expect(s.client.getSnapshot().views.A).toBe(current);
    expect(current.snapshot?.messages[0].id).toBe('fresh');
  });
  it('repairs resyncRequired with snapshot then subscribe at the authoritative cursor', async () => {
    const s = server(); await s.attached('A'); s.cursor = 9007199254740993n;
    s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: s.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
    await s.client.refresh('A');
    const subscriptions = s.requests.filter(item => item.request.method === 'session/subscribe');
    expect(subscriptions.at(-1)?.request.params).toMatchObject({ after_cursor: '9007199254740993' });
    expect(s.client.getSnapshot().views.A.attachment).toBe('attached');
  });
  it.each(['approval', 'questionnaire'] as const)('%s survives disconnect, fresh page client, and publication while absent', async type => {
    const s = server(); const item = interaction(type); s.snapshots.get('A')!.pending_interactions = [item];
    await s.attached('A'); s.client.disconnect();
    expect(s.client.getSnapshot().views.A.snapshot?.pending_interactions).toEqual([item]);
    await s.connect(); expect(s.client.getSnapshot().views.A.snapshot?.pending_interactions).toEqual([item]);
    s.client.disconnect();
    const fresh = server(); fresh.snapshots = s.snapshots; fresh.client.restoreViews(['A']); await fresh.connect();
    expect(fresh.client.getSnapshot().views.A.snapshot?.pending_interactions).toEqual([item]);
    fresh.client.disconnect();
    const absent = interaction(type, 'A', 'created-while-absent'); fresh.snapshots.get('A')!.pending_interactions = [absent];
    await fresh.connect(); expect(fresh.client.getSnapshot().views.A.snapshot?.pending_interactions).toEqual([absent]);
    await fresh.client.answer('A', absent.interaction, type === 'approval' ? { type, decision: { type: 'allow' } } : { type, response: { type: 'declined' } });
    expect(fresh.client.getSnapshot().views.A.snapshot?.pending_interactions).toEqual([]);
  });
  it.each([
    { method: 'session/create', params: { settings: { cwd: '/workspace/A' } } },
    { method: 'session/delete', params: { session_id: 'A', expected_target_revision: 'revision' } },
  ] as const)('never replays a lost $method response', async operation => {
    const s = server(); await s.connect(); s.held.add(operation.method);
    const pending = s.client.request(operation, operation.method === 'session/create' ? 'session_transition' : 'deletion');
    const rejected = expect(pending).rejects.toBeInstanceOf(OutcomeUncertain);
    s.socket.close(); await rejected; await s.connect();
    expect(s.requests.filter(item => item.request.method === operation.method)).toHaveLength(1);
    expect(s.client.getSnapshot().uncertain[0].method).toBe(operation.method);
  });
  it('a timed-out turn acknowledgement closes transport, keeps uncertainty, and does not replay', async () => {
    const s = server(); await s.attached('A'); s.held.add('turn/start'); vi.useFakeTimers();
    const rejected = expect(s.client.send('A', 'run once')).rejects.toBeInstanceOf(OutcomeUncertain);
    await vi.advanceTimersByTimeAsync(30_000); await rejected;
    expect(s.client.getSnapshot().connection).toBe('stale');
    await s.connect(); expect(s.requests.filter(item => item.request.method === 'turn/start')).toHaveLength(1);
    expect(s.client.getSnapshot().uncertain[0].method).toBe('turn/start');
  });
  it('lost interaction acknowledgement never fabricates settlement; native resolution removes controls without cross-talk', async () => {
    const s = server(); const a = interaction('approval'); const b = interaction('questionnaire', 'B');
    s.snapshots.get('A')!.pending_interactions = [a]; s.snapshots.get('B')!.pending_interactions = [b];
    await s.attached('A', 'B'); s.held.add('interaction/respond');
    const rejected = expect(s.client.answer('A', a.interaction, { type: 'approval', decision: { type: 'allow' } })).rejects.toBeInstanceOf(OutcomeUncertain);
    s.socket.close(); await rejected; await s.connect();
    const key = interactionKey(a.interaction);
    expect(s.client.getSnapshot().interactionOperations[key].status).toBe('uncertain');
    expect(s.client.getSnapshot().views.A.snapshot?.pending_interactions).toEqual([a]);
    await expect(s.client.answer('A', a.interaction, { type: 'approval', decision: { type: 'allow' } })).rejects.toThrow('uncertain');
    await s.client.refresh('B'); expect(s.client.getSnapshot().interactionOperations[key].status).toBe('uncertain');
    await s.update('A', { ...snapshot(), pending_interactions: [] });
    expect(s.client.getSnapshot().interactionOperations[key]).toBeUndefined();
    expect(s.client.getSnapshot().uncertain).toEqual([]);
    expect(s.client.getSnapshot().views.B.snapshot?.pending_interactions).toEqual([b]);
    expect(s.requests.filter(item => item.request.method === 'interaction/respond')).toHaveLength(1);
  });
  it('observes actual wire requests/responses/notifications before adaptation', async () => {
    const s = server(); await s.attached('A'); await s.update('A', snapshot());
    const log = s.client.log.getSnapshot();
    expect(log.entries.some(entry => entry.kind === 'notification' && JSON.parse(entry.json).method === 'session/event')).toBe(true);
    expect(log.entries.some(entry => entry.kind === 'response' && entry.method === 'session/attach' && entry.sessionId === 'A')).toBe(true);
    expect(log.entries.filter(entry => entry.direction === 'out').map(entry => JSON.parse(entry.json))).toEqual(s.requests.map(item => item.request));
  });
  it('close view releases attachment and preserves runtime for reopen', async () => {
    const s = server(); await s.attached('A'); await s.client.release('A');
    expect(s.client.getSnapshot().views.A.attachment).toBe('detached');
    expect(s.loaded.has('A')).toBe(true);
    await s.client.attach('A');
    expect(s.client.getSnapshot().views.A.attachment).toBe('attached');
    expect(s.coldLoads.get('A')).toBe(1);
  });
  it('does not deliver a response continuation after its connection has been replaced', async () => {
    const s = server(); await s.connect(); s.held.add('settings/read');
    const read = s.client.request({ method: 'settings/read', params: { session_id: 'A' } }, 'settings');
    const rejected = expect(read).rejects.toThrow('Obsolete connection');
    s.reply(s.requests.at(-1)!.request); s.client.disconnect(); await rejected;
    expect(s.client.getSnapshot().uncertain).toEqual([]);
  });
  it('repairs a notification interleaved before its attach response with a fresh snapshot and subscription', async () => {
    const s = server(); await s.connect(); s.held.add('session/attach');
    const work = s.client.attach('A'); const request = await s.waitFor('session/attach', 1);
    const attached = s.commit(request); const target = s.target('A');
    s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '0', earliest_serviceable: '1' } });
    s.snapshots.set('A', { ...snapshot(), pending_interactions: [interaction('approval')] }); s.cursor = 1n;
    s.socket.deliver(attached); await work;
    expect(s.client.getSnapshot().views.A.snapshot?.pending_interactions).toHaveLength(1);
    expect(s.requests.at(-1)?.request).toMatchObject({ method: 'session/subscribe', params: { target, after_cursor: '1' } });
  });
  it('discards a capacity-queued interaction without marking an unsent response uncertain', async () => {
    const s = server(); s.snapshots.get('A')!.pending_interactions = [interaction('approval')]; await s.attached('A');
    s.held.add('settings/read');
    const reads = Array.from({ length: 8 }, () => s.client.request({ method: 'settings/read', params: { session_id: 'A' } }, 'settings').catch(() => {}));
    const response = s.client.answer('A', interaction('approval').interaction, { type: 'approval', decision: { type: 'allow' } });
    const rejected = expect(response).rejects.not.toBeInstanceOf(OutcomeUncertain);
    expect(s.requests.filter(item => item.request.method === 'interaction/respond')).toEqual([]);
    s.socket.close(); await Promise.all([...reads, rejected]);
    expect(s.client.getSnapshot().uncertain).toEqual([]);
    expect(s.client.getSnapshot().interactionOperations).toEqual({});
  });
  it('pausing raw-log presentation does not pause authoritative protocol processing', async () => {
    const s = server(); await s.attached('A'); s.client.log.pause(true);
    const frozen = s.client.log.getSnapshot().entries;
    await s.update('A', { ...snapshot(), pending_interactions: [interaction('questionnaire')] });
    expect(s.client.getSnapshot().views.A.snapshot?.pending_interactions).toHaveLength(1);
    expect(s.client.log.getSnapshot().entries).toBe(frozen);
    s.client.log.pause(false); expect(s.client.log.getSnapshot().entries.length).toBeGreaterThan(frozen.length);
  });
  it('lost close acknowledgement preserves released intent and never reattaches or replays', async () => {
    const s = server(); await s.attached('A', 'B'); const method = 'session/detach' as const;
    s.held.add(method); const baseline = s.requests.length;
    const release = s.client.release('A');
    expect(s.client.getSnapshot().views.A).toMatchObject({ attachmentIntent: 'released', attachment: 'attached' });
    const rejected = expect(release).rejects.toBeInstanceOf(OutcomeUncertain);
    const request = await s.waitFor(method, 1); s.commit(request);
    expect(s.loaded.has('A')).toBe(true); expect(s.claims().map(item => item.session_id)).toEqual(['B']);
    s.socket.close(); await rejected;
    expect(s.client.getSnapshot().views.A).toMatchObject({ attachmentIntent: 'released', attachment: 'stale' });
    await s.connect();
    expect(s.requests.slice(baseline).filter(({ request }) => request.method === 'session/attach').map(({ request }) => request.params)).toEqual([{ session_id: 'B' }]);
    expect(s.requests.filter(({ request }) => request.method === method)).toHaveLength(1);
    expect(s.loaded.has('A')).toBe(true); expect(s.coldLoads.get('A')).toBe(1);
    expect(s.client.getSnapshot().uncertain).toEqual([expect.objectContaining({ method, sessionId: 'A' })]);
    await s.client.attach('A');
    expect(s.client.getSnapshot().views.A).toMatchObject({ attachmentIntent: 'wanted', attachment: 'attached' });
    expect(s.coldLoads.get('A')).toBe(1);
    expect(s.requests.slice(baseline).filter(({ request }) => request.method === 'session/attach' && request.params.session_id === 'A')).toHaveLength(1);
    // A new explicit attachment cannot prove the old request's outcome.
    expect(s.client.getSnapshot().uncertain).toHaveLength(1);
  });
  it('more than 32 historical open/close owners retain only live external claims', async () => {
    const s = server(); await s.attached('B');
    for (let i = 0; i < 40; i++) {
      const id = `closed-${i}`; s.snapshots.set(id, snapshot(id));
      await s.client.attach(id); expect(s.claims().map(item => item.session_id)).toEqual(['B', id]);
      await s.client.release(id); expect(s.claims().map(item => item.session_id)).toEqual(['B']);
      expect(s.loaded.has(id)).toBe(true);
    }
    expect(s.maxClaims).toBe(2);
    const baseline = s.requests.length; await s.connect();
    expect(s.requests.slice(baseline).filter(({ request }) => request.method === 'session/attach').map(({ request }) => request.params)).toEqual([{ session_id: 'B' }]);
  });
  it('close during attach waits for its exact target, then detaches without changing runtime facts', async () => {
    const s = server(); await s.connect(); s.held.add('session/attach');
    const opening = s.client.attach('A'); const request = await s.waitFor('session/attach', 1);
    const closing = s.client.release('A');
    expect(s.client.getSnapshot().views.A.attachmentIntent).toBe('released');
    s.reply(request); await Promise.all([opening, closing]);
    expect(s.claims()).toEqual([]); expect(s.loaded.has('A')).toBe(true);
    expect(s.requests.filter(({ request }) => request.method === 'session/detach')).toHaveLength(1);
    await s.connect(); expect(s.claims()).toEqual([]);
  });
  it('explicit reopen waits for a pending detach and acquires one fresh authoritative attachment', async () => {
    const s = server(); await s.attached('A'); s.held.add('session/detach');
    const closing = s.client.release('A'); const request = await s.waitFor('session/detach', 1);
    const oldTarget = s.client.target('A');
    const opening = s.client.attach('A'); const duplicate = s.client.attach('A');
    expect(s.client.getSnapshot().views.A.attachmentIntent).toBe('wanted');
    expect(s.requests.filter(({ request }) => request.method === 'session/attach')).toHaveLength(1);
    s.snapshots.get('A')!.pending_interactions = [interaction('questionnaire')];
    s.reply(request); await Promise.all([closing, opening, duplicate]);
    expect(s.requests.filter(({ request }) => request.method === 'session/attach')).toHaveLength(2);
    expect(s.claims()).toHaveLength(1); expect(s.client.target('A')).not.toEqual(oldTarget);
    expect(s.client.getSnapshot().views.A.snapshot?.pending_interactions).toHaveLength(1);
  });
  it('reconnect reads intent after each await instead of restoring a captured historical intent', async () => {
    const s = server(); s.client.restoreViews(['A', 'B']); s.held.add('session/attach');
    const connecting = s.connect(); const request = await s.waitFor('session/attach', 1);
    await s.client.release('B'); s.reply(request); await connecting;
    expect(s.requests.filter(({ request }) => request.method === 'session/attach').map(({ request }) => request.params)).toEqual([{ session_id: 'A' }]);
    expect(s.client.getSnapshot().views.B.attachmentIntent).toBe('released');
  });
  it('a rejected release does not restore attachment intent or claim successful detach', async () => {
    const s = server(); await s.attached('A'); s.held.add('session/detach');
    const rejected = expect(s.client.release('A')).rejects.toThrow('Invalid state');
    const request = await s.waitFor('session/detach', 1);
    s.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Invalid state', data: { kind: 'invalid_state' } } });
    await rejected; expect(s.client.getSnapshot().views.A).toMatchObject({ attachmentIntent: 'released', attachment: 'attached' });
    await s.connect(); expect(s.claims()).toEqual([]);
  });
  it('rejects unsafe endpoint credential paths before opening a socket', async () => {
    const s = server(); await expect(s.client.connect(`${endpoint}?token=secret`, TOKEN)).rejects.toThrow('no credentials');
    expect(s.sockets).toEqual([]);
  });
});

describe('exact inbound mutation transport repair', () => {
  const expected = { sequence: '4', message_id: 'pending-4', revision: '0' };
  const pending = (text: string, revision = '0'): RuntimeClientSnapshot => ({ ...snapshot('A'), inbound: { pending: [{ sequence: '4', revision, message: { id: 'pending-4', source: 'human', content: [{ type: 'text', text }] } }] } });
  it('waits for a post-response snapshot rather than projecting success', async () => {
    const s = server(); s.snapshots.set('A', pending('old')); await s.attached('A');
    s.handlers.set('inbound/edit', () => { s.snapshots.set('A', pending('edited', '1')); return { type: 'inbound_mutation', outcome: { status: 'applied' } }; });
    s.held.add('session/snapshot');
    const work = s.client.editInbound('A', expected, 'edited');
    const read = await s.waitFor('session/snapshot');
    expect(s.client.getSnapshot().views.A.snapshot?.inbound.pending?.[0].revision).toBe('0');
    s.reply(read); expect(await work).toEqual({ status: 'known', outcome: { status: 'applied' }, observed: true });
    expect(s.client.getSnapshot().views.A.snapshot?.inbound.pending?.[0].revision).toBe('1');
  });
  it('lost committed remove is uncertain, never replayed, and repaired on reconnect', async () => {
    const s = server(); s.snapshots.set('A', pending('remove')); await s.attached('A');
    s.handlers.set('inbound/remove', () => { s.snapshots.set('A', snapshot('A')); return { type: 'inbound_mutation', outcome: { status: 'applied' } }; });
    s.held.add('inbound/remove');
    const work = s.client.removeInbound('A', expected);
    const request = await s.waitFor('inbound/remove', 1); s.commit(request);
    s.socket.close(); expect(await work).toEqual({ status: 'uncertain' });
    await s.connect();
    expect(s.client.getSnapshot().views.A.snapshot?.inbound.pending ?? []).toEqual([]);
    expect(s.requests.filter(item => item.request.method === 'inbound/remove')).toHaveLength(1);
  });
  it('late mutation response cannot update a newly attached Session', async () => {
    const s = server(); s.snapshots.set('A', pending('old')); await s.attached('A', 'B');
    s.held.add('inbound/edit');
    const work = s.client.editInbound('A', expected, 'old connection draft');
    const request = await s.waitFor('inbound/edit', 1); const old = s.socket;
    old.close(); expect(await work).toEqual({ status: 'uncertain' });
    await s.connect();
    old.success(request, { type: 'inbound_mutation', outcome: { status: 'applied' } });
    expect(s.client.getSnapshot().views.B.snapshot?.conversation_id).toBe('conversation-B');
    expect(s.client.getSnapshot().views.A.snapshot?.inbound.pending?.[0].revision).toBe('0');
    expect(s.requests.filter(item => item.request.method === 'inbound/edit')).toHaveLength(1);
  });
});


it.each(['not_found', 'committed_cleanup_pending', 'committed_durability_uncertain', 'preview'] as const)('lost delete response is read on reconnect without mutation replay: %s', async status => {
  const s = server(); await s.attached('A');
  s.held.add('session/delete');
  const result = s.client.deleteSession('A', 'confirmed');
  const rejected = expect(result).rejects.toBeInstanceOf(OutcomeUncertain);
  await s.waitFor('session/delete', 1);
  await expect(s.client.send('A', 'must not enter', [], 'send')).rejects.toThrow();
  s.socket.close(); await rejected;
  s.handlers.set('session/deletePreview', () => ({ type: 'deletion', result: status === 'preview'
    ? { status: 'preview', preview: { session_id: 'A', name: null, target_revision: 'fresh', owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0 } }
    : { status, session_id: 'A' } }));
  const before = s.requests.length;
  await s.connect();
  expect(s.requests.filter(row => row.request.method === 'session/delete')).toHaveLength(1);
  expect(s.requests.slice(before).filter(row => row.request.method === 'session/deletePreview')).toHaveLength(1);
  expect(s.requests.slice(before).filter(row => row.request.method === 'session/attach')).toHaveLength(status === 'preview' ? 1 : 0);
  if (status === 'not_found') expect(s.client.getSnapshot().views.A).toBeUndefined();
  if (status === 'committed_cleanup_pending' || status === 'committed_durability_uncertain') expect(s.client.getSnapshot().views.A.deletionRecovery).toBe(status);
  if (status === 'committed_durability_uncertain') expect(s.client.getSnapshot().views.A.deleting).toBe(true);
});


it('lost recovery reply removes recovery authority until fresh committed observation and never replays', async () => {
  const s = server(); await s.attached('A');
  s.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'committed_durability_uncertain', session_id: 'A' } }));
  await s.client.deleteSession('A', 'confirmed');
  s.held.add('session/recoverDeletion');
  const pending = s.client.recoverSessionDeletion('A');
  const lost = expect(pending).rejects.toBeInstanceOf(OutcomeUncertain);
  await s.waitFor('session/recoverDeletion', 1);
  s.socket.close(); await lost;
  expect(s.client.getSnapshot().views.A.deletionRecovery).toBeUndefined();
  await expect(s.client.recoverSessionDeletion('A')).rejects.toThrow('Observe committed');
  s.handlers.set('session/deletePreview', () => ({ type: 'deletion', result: { status: 'committed_cleanup_pending', session_id: 'A' } }));
  await s.connect();
  expect(s.client.getSnapshot().views.A.deletionRecovery).toBe('committed_cleanup_pending');
  expect(s.requests.filter(r => r.request.method === 'session/recoverDeletion')).toHaveLength(1);
  expect(s.requests.filter(r => r.request.method === 'session/delete')).toHaveLength(1);
});

it.each(['preview', 'blocked', 'stale', 'not_found'] as const)('recovery settles authoritative %s without retaining committed authority', async status => {
  const s = server(); await s.attached('A');
  s.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'committed_cleanup_pending', session_id: 'A' } }));
  await s.client.deleteSession('A', 'confirmed');
  s.handlers.set('session/recoverDeletion', () => ({ type: 'deletion', result: status === 'preview'
    ? { status, preview: { session_id: 'A', target_revision: 'fresh', owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0 } }
    : status === 'blocked' ? { status, session_id: 'A', reason: { kind: 'workspace', resource_count: 1 } }
    : { status, session_id: 'A' } }));
  expect((await s.client.recoverSessionDeletion('A'))?.status).toBe(status);
  expect(s.client.getSnapshot().views.A?.deletionRecovery).toBeUndefined();
  if (status === 'not_found') expect(s.client.getSnapshot().views.A).toBeUndefined();
  else expect(s.client.getSnapshot().views.A.deleting).toBe(false);
});

it('T12 native configuration notifications reject stale versions independently per Session', async () => {
  const s = server(); await s.attached('A', 'B');
  const desired = { input_revision: 'input', attempt: '9007199254740993' };
  const application = { scope: 'A', version: '9007199254740993', desired, units: { execution_policy: { status: 'applied' as const } }, candidate: null };
  s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application } });
  s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: { ...application, version: '9007199254740992', units: { execution_policy: { status: 'preparing' } } } } });
  s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: { ...application, scope: 'B', version: '2' } } });
  expect(s.client.getSnapshot().configuration?.A).toEqual(application);
  expect(s.client.getSnapshot().configuration?.B?.version).toBe('2');
  expect(s.requests.filter(({ request }) => ['configuration/sourceWrite', 'configuration/reconcile', 'session/adoptConfiguration'].includes(request.method))).toHaveLength(0);
});
