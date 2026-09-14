import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v1';
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
    expect(s.requests[0].request).toMatchObject({ method: 'initialize', params: { protocol_version: 1 } });
    expect(JSON.stringify(s.client.log.getSnapshot())).not.toContain(TOKEN);
  });
  it('rejects incompatible versions and missing native capabilities', async () => {
    const s = server(); s.version = 9;
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
  it('supports explicit detach and unload only as gestures, then cold attach', async () => {
    const s = server(); await s.attached('A'); await s.client.release('A', false);
    expect(s.client.getSnapshot().views.A.attachment).toBe('detached');
    await s.client.attach('A'); await s.client.release('A', true);
    expect(s.client.getSnapshot().views.A.attachment).toBe('unloaded'); await s.client.attach('A');
    expect(s.client.getSnapshot().views.A.attachment).toBe('attached');
  });
  it('accepts an unload acknowledgement after the native attachment-closed notification', async () => {
    const s = server(); await s.attached('A'); s.held.add('session/unload');
    const target = s.client.target('A'); const unloading = s.client.release('A', true);
    const request = await s.waitFor('session/unload', 1);
    s.socket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target } });
    expect(s.client.getSnapshot().views.A.attachment).toBe('stale');
    s.reply(request); await unloading;
    expect(s.client.getSnapshot().views.A.attachment).toBe('unloaded');
    expect(s.client.getSnapshot().views.A.wanted).toBe(false);
  });
  it('fences retired attachment work and a late unload acknowledgement within the same connection', async () => {
    const s = server(); await s.attached('A'); s.held.add('session/snapshot'); s.held.add('session/unload');
    const oldTarget = s.client.target('A'); const staleRead = s.client.refresh('A');
    const read = await s.waitFor('session/snapshot', 1); const unloading = s.client.release('A', true);
    const unload = await s.waitFor('session/unload', 1);
    s.socket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: oldTarget } });
    await s.client.attach('A'); const newTarget = s.client.target('A');
    expect(newTarget.attachment_id).not.toBe(oldTarget.attachment_id);
    s.held.delete('session/snapshot');
    await s.update('A', { ...snapshot(), messages: [{ id: 'new-incarnation', role: 'assistant', content: [{ type: 'text', text: 'Current' }] }] });
    s.socket.success(read, { type: 'snapshot', snapshot: snapshot(), cursor: '999' });
    s.reply(unload); await Promise.all([staleRead, unloading]);
    expect(s.client.getSnapshot().views.A.target).toEqual(newTarget);
    expect(s.client.getSnapshot().views.A.snapshot?.messages[0].id).toBe('new-incarnation');
    expect(s.client.getSnapshot().views.A.attachment).toBe('attached');
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
    const target = { session_id: 'A', conversation_id: 'conversation-A', runtime_incarnation: '1', attachment_id: 'new-attachment' };
    s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '0', earliest_serviceable: '1' } });
    s.snapshots.set('A', { ...snapshot(), pending_interactions: [interaction('approval')] }); s.cursor = 1n;
    s.socket.success(request, { type: 'attached', target, snapshot: snapshot(), cursor: '0' }); await work;
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
  it('native unload failure retires its route without claiming successful shutdown', async () => {
    const s = server(); await s.attached('A'); s.held.add('session/unload');
    const failure = expect(s.client.release('A', true)).rejects.toThrow('Operation failed');
    const request = await s.waitFor('session/unload', 1);
    s.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Operation failed', data: { kind: 'operation_failed' } } });
    await failure; expect(s.client.getSnapshot().views.A.attachment).toBe('stale');
    expect(s.client.getSnapshot().views.A.target).toBeUndefined();
    expect(s.client.getSnapshot().uncertain).toEqual([]);
  });
  it('rejects unsafe endpoint credential paths before opening a socket', async () => {
    const s = server(); await expect(s.client.connect(`${endpoint}?token=secret`, TOKEN)).rejects.toThrow('no credentials');
    expect(s.sockets).toEqual([]);
  });
});
