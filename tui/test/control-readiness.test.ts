import assert from 'node:assert/strict';
import { it } from 'node:test';
import { AppServerSession } from '../src/app-server/session.ts';
import { harness, nextRequest } from './support/app-server-harness.ts';
import { approvalInteraction, attemptView, checkpoint, domainReceipt, snapshot } from './support/fixtures.ts';

async function refusedEffects(session: AppServerSession) {
  const interaction = approvalInteraction().interaction;
  const effects: (() => Promise<unknown>)[] = [
    () => session.submitInbound([{ type: 'text', text: 'held input' }]),
    () => session.steer([{ type: 'text', text: 'held steering' }]),
    () => session.upload('held.txt', new Uint8Array([1])),
    () => session.cancelCurrentAttempt(),
    () => session.cancelInteraction(interaction),
    () => session.compactContext(),
    () => session.cancelJob('job-held'),
    () => session.sendMessage('agent-held', 'held message'),
    () => session.interruptAgent('agent-held'),
  ];
  for (const effect of effects) await assert.rejects(effect(), /observation is not following/);
  const modelRefusal = session.released ? /Session attachment is unavailable/ : /observation is not following/;
  await assert.rejects(session.modelSet({ model: 'alpha/model-a' }), modelRefusal);
  await assert.rejects(session.modelSelect({ binding_revision: '0', model_position: 0, profile_position: null }), modelRefusal);
}

it('a prepared Session refuses effects until its exact initial subscription receipt', async () => {
  const h = await harness();
  try {
    let prepared: AppServerSession | undefined;
    const currentPrepared = () => prepared;
    const phases: string[] = [];
    const target = { ...h.target, attachment_id: 'prepared-attachment' };
    const attaching = AppServerSession.attach(h.client, target.session_id, undefined, session => {
      prepared = session;
      session.onState(() => phases.push(session.observationPhase));
    });
    const attach = await nextRequest(h, 'session/attach');
    h.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    const read = await nextRequest(h, 'session/checkpoint');
    assert.equal(currentPrepared(), undefined, 'a held checkpoint exposes no prepared Session');
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot({ attempt: attemptView() }), '2') });
    const subscribe = await nextRequest(h, 'session/subscribe');
    const selected = currentPrepared(); assert.ok(selected);
    assert.equal(selected.observationPhase, 'subscribePending');
    const records = h.transport.log.requests.length;
    await refusedEffects(selected);
    assert.equal(h.transport.log.requests.length, records, 'known-unsent refusals dispatch neither effects nor repair reads');
    h.transport.respond(subscribe.id, { type: 'subscribed', target, registration_id: 'prepared-registration', after_cursor: '2' });
    const configuration = await nextRequest(h, 'session/configuration');
    h.transport.respond(configuration.id, { type: 'session_configuration', projection: null });
    const session = await attaching;
    assert.equal(session, prepared); assert.equal(session.observationPhase, 'following');
    assert.deepEqual(phases, ['following'], 'an empty checkpoint still publishes receipt readiness');
    const sending = session.submitInbound([{ type: 'text', text: 'confirmed input' }]);
    const start = await nextRequest(h, 'turn/start');
    assert.equal(start.method, 'turn/start');
    if (start.method !== 'turn/start') assert.fail('actual TurnStart');
    assert.deepEqual(start.params.target, target);
    h.transport.respond(start.id, { type: 'inbound_accepted', message_id: 'original-message', inbound_sequence: '1' });
    assert.deepEqual(await sending, { messageId: 'original-message', sequence: '1' });
    assert.equal(h.transport.log.count('turn/start'), 1);
    assert.equal(h.transport.log.count('session/attach'), 2);
  } finally { h.client.close(); }
});

it('repair keeps effects unavailable through held checkpoint and receipt while reads remain independent', async () => {
  const h = await harness(snapshot({ attempt: attemptView() }));
  try {
    const phases: string[] = [];
    h.session.onState(() => phases.push(h.session.observationPhase));
    const repairing = h.session.resync();
    const read = await nextRequest(h, 'session/checkpoint');
    const records = h.transport.log.requests.length;
    await refusedEffects(h.session); assert.equal(h.transport.log.requests.length, records);
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot({ attempt: attemptView() }), '2') });
    const subscribe = await nextRequest(h, 'session/subscribe');
    assert.equal(h.session.observationPhase, 'subscribePending');
    const pendingRecords = h.transport.log.requests.length;
    await refusedEffects(h.session); assert.equal(h.transport.log.requests.length, pendingRecords);
    const reading = h.session.capabilitiesPage();
    const resources = await nextRequest(h, 'resources/read');
    h.transport.respond(resources.id, domainReceipt('capabilities', []));
    assert.equal((await reading).domain, 'capabilities');
    h.transport.respond(subscribe.id, { type: 'subscribed', target: h.target, registration_id: 'repair-registration', after_cursor: '2' });
    await repairing; assert.equal(h.session.observationPhase, 'following');
    assert.equal(phases.at(-1), 'following', 'receipt alone publishes readiness without waiting for an event');
    const sending = h.session.steer([{ type: 'text', text: 'confirmed steering' }]);
    const steer = await nextRequest(h, 'turn/steer');
    h.transport.respond(steer.id, { type: 'inbound_accepted', message_id: 'original-steering', inbound_sequence: '2' });
    await sending; assert.equal(h.transport.log.count('turn/steer'), 1);
  } finally { h.client.close(); }
});

it('a lost initial receipt never enables effects from the retained prepared checkpoint', async () => {
  const h = await harness();
  try {
    let prepared: AppServerSession | undefined;
    const target = { ...h.target, attachment_id: 'lost-receipt-attachment' };
    const attaching = AppServerSession.attach(h.client, target.session_id, undefined, session => { prepared = session; });
    const attach = await nextRequest(h, 'session/attach');
    h.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    const read = await nextRequest(h, 'session/checkpoint');
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot({ attempt: attemptView() }), '2') });
    await nextRequest(h, 'session/subscribe'); assert.ok(prepared);
    const before = prepared.state;
    h.transport.fail('input_eof'); await attaching;
    assert.equal(prepared.state, before); assert.equal(prepared.state.cursor, '2');
    const records = h.transport.log.requests.length;
    await refusedEffects(prepared); assert.equal(h.transport.log.requests.length, records);
    assert.equal(h.transport.log.count('session/subscribe'), 2);
    assert.equal(h.transport.log.count('turn/start'), 0);
  } finally { h.client.close(); }
});
