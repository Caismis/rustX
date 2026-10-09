import { afterEach, expect, it, vi } from 'vitest';
import { OutcomeUncertain, RequestNotDispatched } from '../src/client/app-server';
import { Server } from './fixture';
const servers: Server[] = [];
afterEach(() => { servers.forEach(s => s.client.disconnect()); vi.restoreAllMocks(); });
async function setup() { const s = new Server(); servers.push(s); await s.attached('A'); return s; }
const count = (s: Server, method: string) => s.requests.filter(row => row.request.method === method).length;
it('diagnostic Release runs after turn socket invocation, with one correlated native outcome', async () => {
  const s = await setup(), proof = s.client.getSnapshot().views.A.attachmentObservation;
  s.held.add('turn/start'); s.held.add('session/detach');
  const order: string[] = [], authorized: boolean[] = [], send = s.socket.send.bind(s.socket);
  vi.spyOn(s.socket, 'send').mockImplementation(raw => {
    if (JSON.parse(raw).method === 'turn/start') { order.push('send'); authorized.push(s.client.isAttachmentControlCurrent('A', proof)); }
    send(raw);
  });
  let release!: Promise<void>, notified!: () => void;
  const notification = new Promise<void>(resolve => { notified = resolve; });
  const stop = s.client.log.subscribe(() => {
    if (order.includes('release') || !s.client.log.getSnapshot().entries.some(e => e.direction === 'out' && e.method === 'turn/start')) return;
    order.push('release'); release = s.client.release('A'); void release.catch(() => {}); notified();
  });
  const work = s.client.request({ method: 'turn/start', params: { target: s.target('A'), content: [] } }, 'inbound_accepted');
  void work.catch(() => {});
  await notification; stop(); expect(order).toEqual(['send', 'release']); expect(authorized).toEqual([true]);
  const request = await s.waitFor('turn/start', 1); s.socket.success(request, { type: 'inbound_accepted', message_id: 'accepted', inbound_sequence: '1' });
  expect((await work).message_id).toBe('accepted');
  s.reply(await s.waitFor('session/detach', 1)); await release;
  expect(count(s, 'turn/start')).toBe(1); expect(count(s, 'session/detach')).toBe(1); expect(s.claims()).toHaveLength(0); expect(s.client.getSnapshot().uncertain).toEqual([]);
  const entries = s.client.log.getSnapshot().entries.filter(e => JSON.parse(e.json).id === request.id);
  expect(entries.map(e => e.direction)).toEqual(['out', 'in']);
  expect(entries.every(e => e.generation === proof!.generation && e.method === 'turn/start' && e.sessionId === 'A')).toBe(true);
});
it.each(['capacity', 'validation'] as const)('%s revocation is unsent and frees the exact reservation', async barrier => {
  const s = await setup(); let resume!: () => void, entered!: () => void;
  const gate = new Promise<boolean>(resolve => { resume = () => resolve(true); });
  const ready = new Promise<void>(resolve => { entered = resolve; });
  s.held.add('session/read');
  const blockers = barrier === 'capacity' ? Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session')) : [];
  const work = s.client.request({ method: 'turn/start', params: { target: s.target('A'), content: [] } }, 'inbound_accepted', undefined,
    barrier === 'validation' ? { current: () => true, validate: () => { entered(); return gate; } } : undefined);
  const outcome = work.catch(error => error); if (barrier === 'validation') await ready;
  const release = s.client.release('A'); void release.catch(() => {});
  if (blockers.length) for (const row of s.requests.filter(r => r.request.method === 'session/read').slice(-blockers.length)) s.reply(row.request);
  resume(); await Promise.all(blockers); await release;
  expect(await outcome).toBeInstanceOf(RequestNotDispatched); expect(count(s, 'turn/start')).toBe(0); expect(s.client.getSnapshot().uncertain).toEqual([]);
  s.held.delete('session/read'); await s.client.attach('A'); await s.client.refresh('A');
  expect(s.claims()).toHaveLength(1); expect(count(s, 'session/snapshot')).toBe(1);
});
it.each(['throw', 'close'] as const)('synchronous send %s is one attempted uncertain outcome, not native acceptance', async failure => {
  const s = await setup(), send = s.socket.send.bind(s.socket); let attempts = 0, acknowledgements = 0;
  vi.spyOn(s.socket, 'send').mockImplementation(raw => {
    if (JSON.parse(raw).method !== 'turn/start') return send(raw);
    attempts++; if (failure === 'close') s.socket.close(); throw new Error('send failed');
  });
  const result = await s.client.request({ method: 'turn/start', params: { target: s.target('A'), content: [] } }, 'inbound_accepted', () => acknowledgements++).catch(error => error);
  expect(result).toBeInstanceOf(OutcomeUncertain); expect(attempts).toBe(1); expect(acknowledgements).toBe(0);
  expect(s.client.getSnapshot().uncertain.filter(row => row.method === 'turn/start')).toHaveLength(1);
  expect((s.client as unknown as { pending: Map<string, unknown> }).pending.size).toBe(0);
  expect(count(s, 'turn/start')).toBe(0); await s.connect(); expect(count(s, 'turn/start')).toBe(0);
});
it('throwing diagnostic observers cannot interrupt synchronous response correlation', async () => {
  const s = await setup(), send = s.socket.send.bind(s.socket);
  vi.spyOn(console, 'error').mockImplementation(() => {}); s.held.add('session/read');
  vi.spyOn(s.socket, 'send').mockImplementation(raw => {
    const request = JSON.parse(raw);
    send(raw);
    if (request.method === 'session/read') s.reply(s.requests.at(-1)!.request);
  });
  const stop = s.client.log.subscribe(() => { throw new Error('observer'); });
  const result = await s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session'); stop();
  expect(result.session.id).toBe('A'); expect((s.client as unknown as { pending: Map<string, unknown> }).pending.size).toBe(0);
  const entries = s.client.log.getSnapshot().entries.slice(-2); expect(entries.map(e => e.direction)).toEqual(['out', 'in']);
  expect(JSON.parse(entries[0].json).id).toBe(JSON.parse(entries[1].json).id);
});

it.each(['direct', 'validated'] as const)('reentrant caller predicate at final %s dispatch cannot revoke then authorize', async mode => {
  const s = await setup(), proof = s.client.getSnapshot().views.A.attachmentObservation;
  s.held.add('turn/start'); s.held.add('session/detach');
  let final = false, release: Promise<void> | undefined, resume!: () => void, entered!: () => void;
  const order: string[] = [], ready = new Promise<void>(resolve => { entered = resolve; });
  const gate = new Promise<boolean>(resolve => { resume = () => resolve(true); });
  const transport = s.client as unknown as { sendPending(pending: unknown): void };
  const sendPending = transport.sendPending.bind(s.client);
  vi.spyOn(transport, 'sendPending').mockImplementation(pending => { final = true; try { sendPending(pending); } finally { final = false; } });
  const predicate = () => {
    order.push(final ? 'final-caller' : 'caller');
    if (final && !release) { order.push('release'); release = s.client.release('A'); void release.catch(() => {}); }
    return true;
  };
  const work = s.client.request({ method: 'turn/start', params: { target: s.target('A'), content: [] } }, 'inbound_accepted', undefined,
    mode === 'direct' ? predicate : { current: predicate, validate: () => { entered(); return gate; } });
  const outcome = work.catch(error => error);
  if (mode === 'validated') { await ready; resume(); }
  await s.waitFor('session/detach', 1);
  expect(order.slice(-2)).toEqual(['final-caller', 'release']);
  expect(s.client.isAttachmentControlCurrent('A', proof)).toBe(false);
  expect(count(s, 'turn/start')).toBe(0);
  expect(await outcome).toBeInstanceOf(RequestNotDispatched); expect(s.client.getSnapshot().uncertain).toEqual([]); expect(s.claims()).toHaveLength(1);
  s.reply(await s.waitFor('session/detach', 1)); await release;
  expect(count(s, 'session/detach')).toBe(1); expect(s.claims()).toHaveLength(0);
  expect((s.client as unknown as { pending: Map<string, unknown> }).pending.size).toBe(0);
});

it('a caller releasing A cannot revoke the independent exact proof for B', async () => {
  const s = new Server(); servers.push(s); await s.attached('A', 'B'); s.held.add('session/detach');
  let release: Promise<void> | undefined; const target = s.target('B');
  const result = await s.client.request({ method: 'turn/start', params: { target, content: [] } }, 'inbound_accepted', undefined, () => {
    if (!release) { release = s.client.release('A'); void release.catch(() => {}); } return true;
  });
  expect(result.type).toBe('inbound_accepted'); expect(count(s, 'turn/start')).toBe(1);
  expect(s.requests.find(r => r.request.method === 'turn/start')!.request.params).toMatchObject({ target });
  s.reply(await s.waitFor('session/detach', 1)); await release; expect(s.claims()).toEqual([target]);
});

it('a sent callback cannot replace correlated native success with a local observer exception', async () => {
  const s = await setup(); s.held.add('turn/start'); let sent = 0, acknowledged = 0;
  vi.spyOn(console, 'error').mockImplementation(() => {});
  const work = s.client.request({ method: 'turn/start', params: { target: s.target('A'), content: [] } }, 'inbound_accepted', () => acknowledged++, {
    current: () => true, validate: async () => true, sent: () => { sent++; throw new Error('local sent observer'); },
  }); void work.catch(() => {});
  const request = await s.waitFor('turn/start', 1); s.reply(request);
  expect((await work).type).toBe('inbound_accepted'); expect(sent).toBe(1); expect(acknowledged).toBe(1);
  expect(s.client.getSnapshot().uncertain).toEqual([]); expect((s.client as unknown as { pending: Map<string, unknown> }).pending.size).toBe(0);
});


it('non-reentrant final validation sends and settles exactly once', async () => {
  const s = await setup(); let validated = 0, sent = 0, acknowledged = 0;
  const result = await s.client.request({ method: 'turn/start', params: { target: s.target('A'), content: [] } }, 'inbound_accepted', () => acknowledged++, {
    current: () => true, validate: async () => { validated++; return true; }, sent: () => { sent++; },
  });
  expect(result.type).toBe('inbound_accepted'); expect([validated, sent, acknowledged]).toEqual([1, 1, 1]);
  expect(count(s, 'turn/start')).toBe(1); expect(s.client.getSnapshot().uncertain).toEqual([]);
  expect((s.client as unknown as { pending: Map<string, unknown> }).pending.size).toBe(0);
});

it('upload freshness revocation after preparation cannot enter the carrier', async () => {
  const s = await setup(); s.held.add('session/uploadPrepare'); s.held.add('session/detach');
  let prepared = false, release: Promise<void> | undefined, acknowledged = 0;
  const work = s.client.upload('A', [new File(['x'], 'x')], {
    current: () => { if (prepared && !release) release = s.client.release('A'); return true; },
    acknowledged: () => acknowledged++,
  }); const outcome = work.catch(error => error);
  const request = await s.waitFor('session/uploadPrepare', 1); prepared = true; s.reply(request);
  expect(await outcome).toMatchObject({ state: 'uncertain' });
  expect(s.carrierTransfers).toBe(0); expect(acknowledged).toBe(0);
  expect(count(s, 'session/uploadPrepare')).toBe(1); expect(count(s, 'session/uploadStatus')).toBe(0);
  s.reply(await s.waitFor('session/detach', 1)); await release;
  expect(s.claims()).toHaveLength(0);
});
