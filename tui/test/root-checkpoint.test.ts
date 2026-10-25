import assert from 'node:assert/strict';
import { it } from 'node:test';
import type { RuntimeClientCheckpoint } from '../../protocol/app-server/v57.ts';
import { AppServerSession } from '../src/app-server/session.ts';
import { harness, nextRequest } from './support/app-server-harness.ts';
import { checkpoint, sessionView, snapshot } from './support/fixtures.ts';

function withSource(value: RuntimeClientCheckpoint): RuntimeClientCheckpoint {
  value.read_cut.source_cut = {
    read: { conversation_id: value.snapshot.conversation_id, journal: value.read_cut.through,
      transcript: '0', mutation_revision: '0', ledger_through: '0', surface_revision: '0',
      transcript_total: '0', pending_through: '0', pending_total: '0' },
    ledger_through: '0', transcript_total: '0', surface_revision: '0', inherited_through: '0',
  };
  return value;
}
const corruptions: [string, (value: RuntimeClientCheckpoint) => void][] = [
  ['snapshot CID', value => { value.snapshot.conversation_id = 'conv_01900000-0000-7000-8000-000000000003'; }],
  ['cut CID', value => { value.read_cut.conversation_id = 'conv_01900000-0000-7000-8000-000000000003'; }],
  ['Child activation', value => { value.read_cut.activation_id = 'child-activation'; }],
  ['cursor revision', value => { value.read_cut.revision = '3'; }],
  ['source CID', value => { value.read_cut.source_cut!.read.conversation_id = 'conv_01900000-0000-7000-8000-000000000003'; }],
  ['source Journal', value => { value.read_cut.source_cut!.read.journal = '3'; }],
  ['source Ledger', value => { value.read_cut.source_cut!.read.ledger_through = '1'; }],
];
for (const [name, corrupt] of corruptions) it(`T13/T17 Root repair rejects ${name} before replacing state or Applied`, async () => {
  const h = await harness(), before = h.session.state, baseline = h.session.checkpoint;
  try {
    const repair = h.session.resync(), rejected = assert.rejects(repair, /checkpoint/i);
    const request = await nextRequest(h, 'session/checkpoint');
    const invalid = withSource(checkpoint(snapshot(), '2')); corrupt(invalid);
    h.transport.respond(request.id, { type: 'checkpoint', checkpoint: invalid }); await rejected;
    assert.equal(h.session.state, before); assert.equal(h.session.checkpoint, baseline);
    assert.equal(h.session.state.cursor, '0'); assert.equal(h.session.resyncCount, 0);
    assert.equal(h.transport.log.matching('session/subscribe').length, 1);
    const validRepair = h.session.resync(), validRead = await nextRequest(h, 'session/checkpoint');
    const valid = withSource(checkpoint(snapshot(), '2'));
    h.transport.respond(validRead.id, { type: 'checkpoint', checkpoint: valid });
    const subscribe = await nextRequest(h, 'session/subscribe');
    assert.equal(subscribe.method, 'session/subscribe');
    if (subscribe.method !== 'session/subscribe') assert.fail('actual Subscribe');
    assert.equal(subscribe.params.after_cursor, '2');
    h.transport.respond(subscribe.id, { type: 'subscribed', target: h.target, registration_id: 'repaired', after_cursor: '2' });
    await validRepair; assert.equal(h.session.state.cursor, '2'); assert.equal(h.session.checkpoint, valid);
    const oldRepair = h.session.resync(), oldRejected = assert.rejects(oldRepair, /predates.*Applied/);
    const oldRead = await nextRequest(h, 'session/checkpoint');
    h.transport.respond(oldRead.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '1') }); await oldRejected;
    assert.equal(h.session.state.cursor, '2'); assert.equal(h.session.checkpoint, valid);
    assert.equal(h.transport.log.matching('session/subscribe').length, 2);
  } finally { h.client.close(); }
});

it('T13 Root initial attach rejects a foreign cut before Subscribe', async () => {
  const h = await harness();
  try {
    const attaching = AppServerSession.attach(h.client, 'ses_84097828-fc31-78c8-9292-10df48901a86'), rejected = assert.rejects(attaching, /checkpoint identity/);
    const attach = await nextRequest(h, 'session/attach');
    const target = { ...h.target, session_id: 'ses_84097828-fc31-78c8-9292-10df48901a86', conversation_id: 'conv_01900000-0000-7000-8000-000000000004', attachment_id: 'second-attachment' };
    h.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    const read = await nextRequest(h, 'session/checkpoint'), invalid = checkpoint(snapshot({ conversation_id: target.conversation_id }));
    invalid.read_cut.conversation_id = 'conv_01900000-0000-7000-8000-000000000003';
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: invalid }); await rejected;
    assert.equal(h.transport.log.matching('session/subscribe').length, 1); assert.equal(h.session.state.cursor, '0');
  } finally { h.client.close(); }
});

function scopedEvent(value: RuntimeClientCheckpoint): import('../../protocol/app-server/v57.ts').RuntimeClientEvent {
  return { type: 'projection_transition', events: [], metadata: { read_cut: value.read_cut, content: [], collections: [] } };
}
const eventCorruptions: [string, (value: RuntimeClientCheckpoint) => import('../../protocol/app-server/v57.ts').RuntimeClientEvent][] = [
  ['foreign snapshot', value => { value.snapshot.conversation_id = 'conv_01900000-0000-7000-8000-000000000003'; return { type: 'checkpoint_advanced', transition: 'scope', checkpoint: value }; }],
  ['foreign metadata', value => { value.read_cut.conversation_id = 'conv_01900000-0000-7000-8000-000000000003'; return scopedEvent(value); }],
  ['Child metadata', value => { value.read_cut.activation_id = 'child-activation'; return scopedEvent(value); }],
  ['metadata cursor', value => { value.read_cut.revision = '2'; return scopedEvent(value); }],
  ['checkpoint cursor', value => { value.cursor = '2'; value.read_cut.revision = '2'; return { type: 'checkpoint_advanced', transition: 'scope', checkpoint: value }; }],
  ['nested checkpoint', value => {
    const inner = checkpoint(snapshot({ conversation_id: 'conv_01900000-0000-7000-8000-000000000003' }), '1');
    return { type: 'projection_transition', events: [{ type: 'checkpoint_advanced', transition: 'scope', checkpoint: inner }], metadata: { read_cut: value.read_cut, content: [], collections: [] } };
  }],
];
for (const [name, corrupt] of eventCorruptions) it(`T14/T17 matching Root target cannot apply ${name} inner event evidence`, async () => {
  const h = await harness(), before = h.session.state, baseline = h.session.checkpoint;
  try {
    h.transport.emit(h.target, '1', corrupt(checkpoint(snapshot(), '1')));
    assert.equal(h.session.state, before); assert.equal(h.session.checkpoint, baseline); assert.equal(h.session.state.cursor, '0');
    const read = await nextRequest(h, 'session/checkpoint');
    assert.equal(h.transport.log.matching('session/subscribe').length, 1);
    const repaired = checkpoint(snapshot(), '2');
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: repaired });
    const subscribe = await nextRequest(h, 'session/subscribe');
    assert.equal(subscribe.method, 'session/subscribe');
    if (subscribe.method !== 'session/subscribe') assert.fail('actual Subscribe');
    assert.equal(subscribe.params.after_cursor, '2');
    h.transport.respond(subscribe.id, { type: 'subscribed', target: h.target, registration_id: 'repaired-events', after_cursor: '2' });
    await h.session.resync();
    assert.equal(h.session.state.cursor, '2');
    const next = checkpoint(snapshot(), '3');
    h.transport.emit(h.target, '3', { type: 'checkpoint_advanced', transition: 'valid', checkpoint: next });
    assert.equal(h.session.state.cursor, '3'); assert.equal(h.session.checkpoint?.read_cut.revision, '3');
  } finally { h.client.close(); }
});

for (const operation of ['detach', 'switch'] as const) {
  for (const acquisition of ['repair', 'attach'] as const) it(`T17 late valid ${acquisition} receipt cannot fold buffered events after ${operation}`, async () => {
    const h = await harness();
    try {
      let session = h.session;
      const target = acquisition === 'repair' ? h.target : { ...h.target, attachment_id: 'late-attachment' };
      const acquiring = acquisition === 'repair' ? session.resync() : AppServerSession.attach(h.client, target.session_id, undefined, value => {
        session = value;
        h.client.onNotification(message => value.applyNotification(message));
      });
      if (acquisition === 'attach') {
        const attach = await nextRequest(h, 'session/attach');
        h.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
      }
      const read = await nextRequest(h, 'session/checkpoint');
      const baseline = checkpoint(snapshot(), '2');
      h.transport.respond(read.id, { type: 'checkpoint', checkpoint: baseline });
      const subscribe = await nextRequest(h, 'session/subscribe');
      assert.equal(session.observationPhase, 'subscribePending');
      h.transport.notify({ jsonrpc: '2.0', method: 'session/event', params: {
        target, registration_id: 'late-registration', cursor: '3',
        event: { type: 'checkpoint_advanced', transition: 'late', checkpoint: checkpoint(snapshot(), '3') },
      } });
      const before = session.state;
      assert.equal(before.cursor, '2', 'the event is buffered before receipt acceptance');
      const releasing = operation === 'detach' ? session.detach() : session.switchNode('node-next');
      const request = await nextRequest(h, operation === 'detach' ? 'session/detach' : 'session/switchNode');
      assert.equal(session.observationPhase, 'retired');
      const records = h.transport.log.requests.length;
      await assert.rejects(session.submitInbound([{ type: 'text', text: 'retired input' }]), /request not sent/);
      await assert.rejects(session.steer([{ type: 'text', text: 'retired steering' }]), /request not sent/);
      assert.equal(h.transport.log.requests.length, records, 'retirement refuses new effects without replay or repair');
      h.transport.respond(subscribe.id, { type: 'subscribed', target, registration_id: 'late-registration', after_cursor: '2' });
      if (operation === 'detach') h.transport.respond(request.id, { type: 'detached' });
      else h.transport.respond(request.id, { type: 'session', session: sessionView() });
      await releasing; await acquiring;
      assert.equal(session.state, before);
      assert.equal(session.checkpoint, baseline);
      assert.equal(session.state.cursor, '2', 'retirement preserves the last Applied baseline');
      assert.equal(session.observationPhase, 'retired');
      assert.equal(session.controlReady, false);
      assert.equal(h.transport.log.matching('session/checkpoint').length, 2);
      assert.equal(h.transport.log.matching('session/configuration').length, 1, 'no post-retirement configuration acquisition');
    } finally { h.client.close(); }
  });
}

it('T17 prepared attach released immediately never starts a subscription', async () => {
  const h = await harness();
  try {
    let releasing: Promise<void> | undefined;
    const attaching = AppServerSession.attach(h.client, h.target.session_id, undefined, value => { releasing = value.detach(); });
    const attach = await nextRequest(h, 'session/attach');
    h.transport.respond(attach.id, { type: 'attached', target: h.target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    const read = await nextRequest(h, 'session/checkpoint');
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '2') });
    const detach = await nextRequest(h, 'session/detach');
    h.transport.respond(detach.id, { type: 'detached' });
    const session = await attaching; await releasing;
    assert.equal(session.observationPhase, 'retired');
    assert.equal(session.state.cursor, '2');
    assert.equal(h.transport.log.matching('session/subscribe').length, 1);
  } finally { h.client.close(); }
});

it('Source unavailability preserves Applied and requires explicit recovery without replay or execution mutation', async () => {
  const h = await harness(), before = h.session.state, baseline = h.session.checkpoint;
  try {
    const unavailable = (registration_id: string) => h.transport.notify({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target:h.target,registration_id,message:'Original source preparation failed'}});
    unavailable('retired-registration');
    assert.equal(h.session.controlReady, true);
    unavailable('harness-registration-1');
    assert.equal(h.session.controlReady, false);
    assert.equal(h.session.state, before); assert.equal(h.session.checkpoint, baseline);
    assert.equal(h.session.state.cursor, '0');
    assert.match(h.session.observationError!, /Original source preparation failed/);
    await assert.rejects(h.session.submitInbound([{type:'text',text:'must stay unsent'}]), /not ready|following|observation/i);
    h.transport.emit(h.target,'1',{type:'message_committed',attempt_id:'old',transcript_cursor:'1',message:{id:'stale',role:'assistant',content:[]}});
    await Promise.resolve();
    assert.equal(h.session.state, before);
    assert.equal(h.transport.log.count('session/checkpoint'),1); assert.equal(h.transport.log.count('session/subscribe'),1);
    assert.equal(h.transport.log.count('turn/start'),0); assert.equal(h.transport.log.count('turn/cancel'),0);
    const repair=h.session.resync(), read=await nextRequest(h,'session/checkpoint');
    h.transport.respond(read.id,{type:'checkpoint',checkpoint:checkpoint(snapshot(),'2')});
    const subscribe=await nextRequest(h,'session/subscribe');
    h.transport.respond(subscribe.id,{type:'subscribed',target:h.target,registration_id:'serviceable-source',after_cursor:'2'});
    await repair;
    assert.equal(h.session.state.cursor,'2'); assert.equal(h.session.controlReady,true);
    unavailable('harness-registration-1'); assert.equal(h.session.controlReady,true);
    h.transport.notify({jsonrpc:'2.0',method:'session/event',params:{target:h.target,registration_id:'serviceable-source',cursor:'3',event:{type:'message_committed',attempt_id:'current',transcript_cursor:'3',message:{id:'current',role:'assistant',content:[]}}}});
    assert.equal(h.session.state.cursor,'3');
    assert.equal(h.transport.log.count('session/checkpoint'),2); assert.equal(h.transport.log.count('session/subscribe'),2);
  } finally { h.client.close(); }
});

it('unavailable buffered before the Subscribe promise continuation stays unavailable without another checkpoint',async()=>{
 const h=await harness();
 try {
  const work=h.session.resync(),refused=assert.rejects(work,/source unavailable/i);
  const read=await nextRequest(h,'session/checkpoint');h.transport.respond(read.id,{type:'checkpoint',checkpoint:checkpoint(snapshot(),'2')});
  const subscribing=await nextRequest(h,'session/subscribe'),before=h.session.state;
  h.transport.respond(subscribing.id,{type:'subscribed',target:h.target,registration_id:'pending-unavailable',after_cursor:'2'});
  h.transport.notify({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target:h.target,registration_id:'pending-unavailable',message:'pending source fault'}});
  assert.equal(h.session.observationPhase,'subscribePending');
  await refused;
  assert.equal(h.session.observationPhase,'unavailable');assert.equal(h.session.state,before);assert.equal(h.session.state.cursor,'2');
  assert.equal(h.session.controlReady,false);assert.match(h.session.observationError!,/source unavailable/i);
  await Promise.resolve();assert.equal(h.transport.log.count('session/checkpoint'),2);assert.equal(h.transport.log.count('session/subscribe'),2);assert.equal(h.transport.log.count('turn/start'),0);
 }finally{h.client.close();}
});
