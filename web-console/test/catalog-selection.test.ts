import { afterEach, beforeEach, expect, it } from 'vitest';
import { Server, domainReceipt } from './fixture';

let server: Server;
const choice = { binding_revision: '7', model_position: 72, profile_position: 71 };
beforeEach(async () => {
  server = new Server(); await server.attached('A');
  server.handlers.set('session/selectModel', request => {
    if (request.method !== 'session/selectModel') throw Error('Wrong method');
    expect(request.params).toEqual({ target: server.target('A'), choice });
    return { type: 'model_set', settings_revision: '9' };
  });
});
afterEach(() => server.client.disconnect());

it('T13/T15 catalog selection sends native generation and ordinals and consumes only a bounded receipt', async () => {
  const before = server.requests.length;
  await server.client.selectCatalogModel('A', choice);
  expect(server.requests.slice(before).map(row => row.request.method)).toEqual(['session/selectModel']);
  expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('acknowledged');
  await expect(server.client.send('A', 'requires authoritative confirmation')).rejects.toThrow('Reread native model state');
  expect(server.requests.some(row => row.request.method === 'turn/start')).toBe(false);
  await server.client.repairAgentModel('A');
  expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined();
  expect(server.requests.filter(row => row.request.method === 'session/selectModel')).toHaveLength(1);
});

it('T11 a retired model chooser cannot dispatch a mutation', async () => {
  const before = server.requests.length;
  await server.client.selectCatalogModel('A', choice, () => false);
  expect(server.requests).toHaveLength(before);
  expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined();
});

it('T13 a lost catalog-selection response preserves uncertainty and is never replayed', async () => {
  server.held.add('session/selectModel');
  const work = server.client.selectCatalogModel('A', choice);
  const rejected = expect(work).rejects.toThrow('Outcome uncertain');
  await server.waitFor('session/selectModel', 1);
  server.socket.close();
  await rejected;
  expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('uncertain');
  await server.connect();
  expect(server.requests.filter(row => row.request.method === 'session/selectModel')).toHaveLength(1);
});

it('T11 an accepted response after closing its chooser still confirms the exact dispatched mutation', async () => {
  server.held.add('session/selectModel');
  let owned = true;
  const work = server.client.selectCatalogModel('A', choice, () => owned);
  const request = await server.waitFor('session/selectModel', 1);
  owned = false;
  server.reply(request);
  await work;
  expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('acknowledged');
  expect(server.requests.filter(row => row.request.method === 'session/selectModel')).toHaveLength(1);
});

it('T15 opening a model picker retains finite headers and leaves growing rows deferred', async () => {
  const result = domainReceipt('model_catalog.models', [null]);
  result.page.items[0].deferred = true;
  server.handlers.set('session/models', () => result);
  const before = server.requests.length;
  const loaded = await server.client.reads.modelCatalogRead(server.target('A'), () => true);
  expect(loaded.catalog.models).toEqual([]);
  expect(loaded.page.items[0].deferred).toBe(true);
  expect(server.requests.slice(before).map(row => row.request.method)).toEqual(['session/models']);
});
