import assert from 'node:assert/strict';
import { it } from 'node:test';
import { harness, nextRequest } from './support/app-server-harness.ts';
import { attemptView, snapshot } from './support/fixtures.ts';

it('native cancellation forwards under stale unavailable observation and returns its actual owner', async () => {
  const h = await harness(snapshot({ attempt: attemptView({ attempt_id: 'old-A', phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } }) }));
  try {
    const before = h.session.state;
    h.transport.notify({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target:h.target,registration_id:'harness-registration-1',message:'held original source'}});
    assert.equal(h.session.observationPhase, 'unavailable');
    const reads = h.transport.log.count('session/checkpoint');
    const cancelling = h.session.cancelCurrentAttempt();
    const request = await nextRequest(h, 'turn/cancel');
    assert.equal(request.method, 'turn/cancel');
    if (request.method !== 'turn/cancel') assert.fail('native current-Attempt control');
    assert.deepEqual(request.params.target, h.target);
    h.transport.respond(request.id, {type:'cancellation_accepted',attempt_id:'actual-B'});
    assert.equal(await cancelling, 'actual-B');
    assert.equal(h.session.state, before);
    assert.equal(h.session.observationPhase, 'unavailable');
    assert.equal(h.transport.log.count('session/checkpoint'), reads);
    assert.equal(h.transport.log.count('turn/cancel'), 1);
    const detaching = h.session.detach();
    const detach = await nextRequest(h, 'session/detach');
    h.transport.respond(detach.id, {type:'detached'}); await detaching;
    await assert.rejects(h.session.cancelCurrentAttempt(), /attachment is unavailable/);
    assert.equal(h.transport.log.count('turn/cancel'), 1);
  } finally { h.client.close(); }
});
