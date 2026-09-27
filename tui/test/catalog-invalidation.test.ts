import assert from 'node:assert/strict';
import { test } from 'node:test';
import { harness, nextRequest } from './support/app-server-harness.ts';

for (const catalog_changed of [false, true]) {
  test(`catalog invalidation ${catalog_changed} preserves explicit scope and read-on-demand catalog ownership`, async t => {
    const h = await harness(); t.after(() => h.client.close());
    const before = h.session.state;
    const scopes: boolean[] = [];
    h.client.onNotification(message => {
      if (message.method === 'session/summaryInvalidated') scopes.push(message.params.catalog_changed);
    });
    h.transport.deliver({ jsonrpc: '2.0', method: 'session/summaryInvalidated', params: { session_id: h.session.sessionId, catalog_changed } });
    assert.deepEqual(scopes, [catalog_changed]);
    assert.equal(h.session.state, before, 'catalog invalidation is not Conversation lifecycle');
    assert.equal(h.transport.log.count('session/list'), 0, 'no retained live catalog to heuristically repair');
    for (let i = 0; i < 2; i++) {
      const listing = h.host.listSessions();
      const request = await nextRequest(h, 'session/list', i);
      h.transport.respond(request.id, { type: 'sessions', sessions: [], next_offset: null });
      assert.deepEqual(await listing, { sessions: [], nextOffset: undefined });
    }
    assert.equal(h.transport.log.count('session/list'), 2, 'every catalog opening reads current native membership');
    assert.equal(h.client.closed, undefined);
  });
}
