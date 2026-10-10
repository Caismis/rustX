import { afterEach, beforeEach, expect, it } from 'vitest';
import { Server } from './fixture';
import { cfg3Effective } from './cfg3-data';
import { RpcFailure } from '../src/client/app-server';
let server: Server;
beforeEach(async () => {
  server = new Server(); await server.connect();
  for (const id of ['A', 'B']) server.snapshots.get(id)!.model = cfg3Effective().effective_model;
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw Error('Wrong method');
    const snapshot = server.snapshots.get(request.params.target.session_id)!;
    snapshot.model = { ...snapshot.model!, configured: request.params.config, effective: { ...snapshot.model!.effective!, model: request.params.config.model, profile: request.params.config.profile } };
    return { type: 'model', model: snapshot.model };
  });
  server.held.add('session/attach');
});
afterEach(() => server.client.disconnect());

it('coalesces model and reasoning choices, and holds send until native acknowledgement and reread', async () => {
  const attach = server.client.attach('A'); const opening = await server.waitFor('session/attach', 1);
  const first = server.client.prepareAgentModel('A', { model: 'first', profile: 'low' });
  const last = server.client.prepareAgentModel('A', { model: 'last', profile: 'high' });
  expect(last).toBe(first);
  server.held.add('session/setModel');
  const send = server.client.send('A', 'after selection');
  server.reply(opening); await attach;
  const mutation = await server.waitFor('session/setModel', 1);
  expect(mutation.params).toMatchObject({ config: { model: 'last', profile: 'high' } });
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
  server.reply(mutation); await first; await send;
  expect(server.client.getSnapshot().views.A.snapshot?.model?.configured).toEqual({ model: 'last', profile: 'high' });
  expect(server.requests.filter(row => row.request.method === 'session/setModel')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

it('a rejected selection preserves failure and never sends with the old model', async () => {
  const attach = server.client.attach('A'); const opening = await server.waitFor('session/attach', 1);
  server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32000, message: 'Model removed' }); });
  const chosen = server.client.prepareAgentModel('A', { model: 'removed' });
  const rejected = expect(chosen).rejects.toThrow('Model removed');
  const send = expect(server.client.send('A', 'must not use old model')).rejects.toThrow('Model removed');
  server.reply(opening); await attach; await rejected; await send;
  expect(server.client.getSnapshot().views.A.modelIntent?.phase).toBe('failed');
  await expect(server.client.send('A', 'still blocked')).rejects.toThrow('Model removed');
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
});

it('connection loss retires the choice and never replays it after reconnect', async () => {
  const attach = server.client.attach('A').catch(() => {}); await server.waitFor('session/attach', 1);
  const chosen = server.client.prepareAgentModel('A', { model: 'old-connection' });
  const rejected = expect(chosen).rejects.toThrow();
  server.socket.close(); await attach; await rejected;
  expect(server.client.getSnapshot().views.A.modelIntent).toBeUndefined();
  server.held.delete('session/attach');
  await server.connect();
  expect(server.requests.filter(row => row.request.method === 'session/setModel')).toHaveLength(0);
});

it('other conversation attachment cannot redirect a pending choice', async () => {
  const a = server.client.attach('A'); const openingA = await server.waitFor('session/attach', 1);
  const chosen = server.client.prepareAgentModel('A', { model: 'only-A' });
  const b = server.client.attach('B'); const openingB = await server.waitFor('session/attach', 2);
  server.reply(openingB); await b;
  expect(server.requests.filter(row => row.request.method === 'session/setModel')).toHaveLength(0);
  server.reply(openingA); await a; await chosen;
  const request = await server.waitFor('session/setModel', 1);
  expect(request.params).toMatchObject({ target: { session_id: 'A' } });
  expect(server.client.getSnapshot().views.B.snapshot?.model?.configured.model).not.toBe('only-A');
});

it('initialization failure retires the pending send without applying any model', async () => {
  server.handlers.set('session/attach', () => { throw new RpcFailure({ code: -32000, message: 'Initialization failed' }); });
  const attach = expect(server.client.attach('A')).rejects.toThrow();
  const opening = await server.waitFor('session/attach', 1);
  const choice = expect(server.client.prepareAgentModel('A', { model: 'chosen' })).rejects.toThrow();
  const send = expect(server.client.send('A', 'hello')).rejects.toThrow();
  server.reply(opening); await attach; await choice; await send;
  expect(server.requests.filter(row => ['session/setModel', 'turn/start'].includes(row.request.method))).toHaveLength(0);
});

it('closing before initialization finishes retires the pending model choice', async () => {
  const attach = server.client.attach('A').catch(() => {});
  const opening = await server.waitFor('session/attach', 1);
  const choice = expect(server.client.prepareAgentModel('A', { model: 'retired' })).rejects.toThrow();
  const release = server.client.release('A');
  server.reply(opening); await attach; await release; await choice;
  expect(server.client.getSnapshot().views.A.modelIntent).toBeUndefined();
  expect(server.requests.filter(row => row.request.method === 'session/setModel')).toHaveLength(0);
});

it('a failed confirmation can recover through reread without replaying the mutation', async () => {
  const attach = server.client.attach('A'); const opening = await server.waitFor('session/attach', 1);
  const apply = server.handlers.get('session/setModel')!;
  server.handlers.set('session/setModel', request => {
    const result = apply(request);
    server.handlers.set('session/snapshot', () => { throw new RpcFailure({ code: -32000, message: 'Read failed' }); });
    return result;
  });
  const choice = expect(server.client.prepareAgentModel('A', { model: 'chosen' })).rejects.toThrow();
  server.reply(opening); await attach; await choice;
  server.handlers.delete('session/snapshot');
  await server.client.refresh('A');
  await server.client.repairAgentModel('A');
  expect(server.client.getSnapshot().views.A.modelIntent).toBeUndefined();
  await server.client.send('A', 'after confirmation');
  expect(server.requests.filter(row => row.request.method === 'session/setModel')).toHaveLength(1);
});

it('a rejected cold choice allows an explicit replacement after rereading', async () => {
  const apply = server.handlers.get('session/setModel')!;
  server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32000, message: 'Removed' }); });
  const attach = server.client.attach('A'); const opening = await server.waitFor('session/attach', 1);
  const choice = expect(server.client.prepareAgentModel('A', { model: 'removed' })).rejects.toThrow();
  server.reply(opening); await attach; await choice;
  await server.client.repairAgentModel('A');
  server.handlers.set('session/setModel', apply);
  await server.client.setAgentModel('A', { model: 'replacement' });
  await server.client.repairAgentModel('A');
  await server.client.send('A', 'retry');
  expect(server.client.getSnapshot().views.A.modelIntent).toBeUndefined();
  expect(server.client.getSnapshot().views.A.snapshot?.model?.configured.model).toBe('replacement');
});
