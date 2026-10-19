import { afterEach, beforeEach, expect, it } from 'vitest';
import { Server, domainReceipt } from './fixture';

let server: Server;
beforeEach(async () => { server = new Server(); await server.attached('A'); });
afterEach(() => server.client.disconnect());

it('T15 opening a model picker retains finite headers and leaves growing rows deferred', async () => {
  const result = domainReceipt('model_catalog.models', [null]);
  result.page.items[0].deferred = true;
  server.handlers.set('session/models', () => result);
  const before = server.requests.length;
  const loaded = await server.client.modelCatalogRead(server.target('A'), () => true);
  expect(loaded.catalog.models).toEqual([]);
  expect(loaded.page.items[0].deferred).toBe(true);
  expect(server.requests.slice(before).map(row => row.request.method)).toEqual(['session/models']);
});
