import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientCheckpoint } from '../../protocol/app-server/v57';
import { checkpoint, Server, snapshot } from './fixture';

const servers: Server[] = [];
afterEach(() => { for (const server of servers) server.client.disconnect(); });
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
  ['snapshot CID', value => { value.snapshot.conversation_id = 'foreign'; }],
  ['cut CID', value => { value.read_cut.conversation_id = 'foreign'; }],
  ['Child activation', value => { value.read_cut.activation_id = 'child-activation'; }],
  ['cursor revision', value => { value.read_cut.revision = '3'; }],
  ['source CID', value => { value.read_cut.source_cut!.read.conversation_id = 'foreign'; }],
  ['source Journal', value => { value.read_cut.source_cut!.read.journal = '3'; }],
  ['source Ledger', value => { value.read_cut.source_cut!.read.ledger_through = '1'; }],
];
it.each(corruptions)('T13/T17 Root repair rejects %s before replacing state or Applied', async (_name, corrupt) => {
  const s = new Server(); servers.push(s); await s.attached('A');
  const baseline = s.client.getSnapshot().views.A;
  const subscriptions = () => s.requests.filter(row => row.request.method === 'session/subscribe').length;
  s.handlers.set('session/checkpoint', () => {
    const invalid = withSource(checkpoint(snapshot(), '2')); corrupt(invalid);
    return { type: 'checkpoint', checkpoint: invalid };
  });
  await expect(s.client.refresh('A')).rejects.toThrow(/checkpoint/i);
  const refused = s.client.getSnapshot().views.A;
  expect(refused.snapshot).toBe(baseline.snapshot); expect(refused.checkpoint).toBe(baseline.checkpoint);
  expect(refused.cursor).toBe('0'); expect(subscriptions()).toBe(1);
  const valid = withSource(checkpoint(snapshot(), '2'));
  s.handlers.set('session/checkpoint', () => ({ type: 'checkpoint', checkpoint: valid }));
  await s.client.refresh('A');
  expect(s.client.getSnapshot().views.A.cursor).toBe('2'); expect(s.client.getSnapshot().views.A.checkpoint).toEqual(valid);
  expect(subscriptions()).toBe(2);
  s.handlers.set('session/checkpoint', () => ({ type: 'checkpoint', checkpoint: checkpoint(snapshot(), '1') }));
  await expect(s.client.refresh('A')).rejects.toThrow(/predates.*Applied/);
  expect(s.client.getSnapshot().views.A.cursor).toBe('2'); expect(s.client.getSnapshot().views.A.checkpoint).toEqual(valid);
  expect(subscriptions()).toBe(2);
});

it('T13 Root initial attach rejects a foreign cut before Subscribe', async () => {
  const s = new Server(); servers.push(s); await s.connect();
  s.handlers.set('session/checkpoint', () => {
    const invalid = checkpoint(snapshot()); invalid.read_cut.conversation_id = 'foreign';
    return { type: 'checkpoint', checkpoint: invalid };
  });
  await expect(s.client.attach('A')).rejects.toThrow(/checkpoint identity/);
  expect(s.client.getSnapshot().views.A.snapshot).toBeUndefined();
  expect(s.client.getSnapshot().views.A.cursor).toBeUndefined();
  expect(s.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(0);
});

function scopedEvent(value: RuntimeClientCheckpoint): import('../../protocol/app-server/v57').RuntimeClientEvent {
  return { type: 'projection_transition', events: [], metadata: { read_cut: value.read_cut, content: [], collections: [] } };
}
const eventCorruptions: [string, (value: RuntimeClientCheckpoint) => import('../../protocol/app-server/v57').RuntimeClientEvent][] = [
  ['foreign snapshot', value => { value.snapshot.conversation_id = 'foreign'; return { type: 'checkpoint_advanced', transition: 'scope', checkpoint: value }; }],
  ['foreign metadata', value => { value.read_cut.conversation_id = 'foreign'; return scopedEvent(value); }],
  ['Child metadata', value => { value.read_cut.activation_id = 'child-activation'; return scopedEvent(value); }],
  ['metadata cursor', value => { value.read_cut.revision = '2'; return scopedEvent(value); }],
  ['checkpoint cursor', value => { value.cursor = '2'; value.read_cut.revision = '2'; return { type: 'checkpoint_advanced', transition: 'scope', checkpoint: value }; }],
  ['nested checkpoint', value => {
    const inner = checkpoint({ ...snapshot(), conversation_id: 'foreign' }, '1');
    return { type: 'projection_transition', events: [{ type: 'checkpoint_advanced', transition: 'scope', checkpoint: inner }], metadata: { read_cut: value.read_cut, content: [], collections: [] } };
  }],
];
it.each(eventCorruptions)('T14/T17 matching Root target cannot apply %s inner event evidence', async (_name, corrupt) => {
  const s = new Server(); servers.push(s); await s.attached('A'); s.held.add('session/checkpoint');
  const before = s.client.getSnapshot().views.A, target = s.target('A');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target, cursor: '1', event: corrupt(checkpoint(snapshot(), '1')) } });
  const refused = s.client.getSnapshot().views.A;
  expect(refused.snapshot).toBe(before.snapshot); expect(refused.checkpoint).toBe(before.checkpoint); expect(refused.cursor).toBe('0');
  const read = await s.waitFor('session/checkpoint', 2);
  expect(s.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(1);
  const repaired = checkpoint(snapshot(), '2');
  const resumed = new Promise<void>(resolve => {
    const stop = s.client.subscribe(() => {
      const view = s.client.getSnapshot().views.A;
      if (view.cursor === '2' && view.observationState === 'following') { stop(); resolve(); }
    });
  });
  s.socket.success(read, { type: 'checkpoint', checkpoint: repaired });
  await resumed; expect(s.client.getSnapshot().views.A.cursor).toBe('2');
  expect(s.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(2);
  const next = checkpoint(snapshot(), '3');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target, cursor: '3', event: { type: 'checkpoint_advanced', transition: 'valid', checkpoint: next } } });
  expect(s.client.getSnapshot().views.A.cursor).toBe('3'); expect(s.client.getSnapshot().views.A.checkpoint).toEqual(next);
});

it('Source failure keeps the native snapshot and Applied without an automatic resync loop', async () => {
  const s=new Server();servers.push(s);await s.attached('A');
  const target=s.target('A'),before=s.client.getSnapshot().views.A;
  const registration=s.registrations.get(JSON.stringify(target))!;
  const count=(method:string)=>s.requests.filter(row=>row.request.method===method).length;
  const unavailable=(registration_id:string)=>s.socket.deliver({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target,registration_id,message:'Original source preparation failed'}});
  unavailable('old-registration');expect(s.client.isAttachmentControlCurrent('A',before.attachmentObservation)).toBe(true);
  unavailable(registration);
  const failed=s.client.getSnapshot().views.A;
  expect(failed.snapshot).toBe(before.snapshot);expect(failed.checkpoint).toBe(before.checkpoint);expect(failed.cursor).toBe(before.cursor);
  expect(failed.observationState).toBe('unavailable');expect(failed.error).toContain('Original source preparation failed');
  expect(s.client.isAttachmentControlCurrent('A',before.attachmentObservation)).toBe(false);
  expect(()=>s.client.target('A')).toThrow();
  s.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target,registration_id:registration,cursor:'1',event:{type:'message_committed',attempt_id:'old',transcript_cursor:'1',message:{id:'old',role:'assistant',content:[]}}}});
  await Promise.resolve();
  expect(s.client.getSnapshot().views.A.snapshot).toBe(before.snapshot);expect(s.client.getSnapshot().views.A.cursor).toBe(before.cursor);
  expect(count('session/checkpoint')).toBe(1);expect(count('session/subscribe')).toBe(1);expect(count('turn/start')).toBe(0);expect(count('turn/cancel')).toBe(0);
  s.handlers.set('session/checkpoint',()=>({type:'checkpoint',checkpoint:checkpoint(snapshot(),'2')}));
  await s.client.refresh('A');expect(s.client.getSnapshot().views.A.cursor).toBe('2');
  expect(s.client.isAttachmentControlCurrent('A',before.attachmentObservation)).toBe(true);
  unavailable(registration);expect(s.client.getSnapshot().views.A.observationState).toBe('following');
  const current=s.registrations.get(JSON.stringify(target))!;
  s.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target,registration_id:current,cursor:'3',event:{type:'message_committed',attempt_id:'current',transcript_cursor:'3',message:{id:'current',role:'assistant',content:[]}}}});
  expect(s.client.getSnapshot().views.A.cursor).toBe('3');expect(count('session/checkpoint')).toBe(2);expect(count('session/subscribe')).toBe(2);
});

it('unavailable queued before the Subscribe receipt continuation remains a source failure without resync',async()=>{
 const s=new Server();servers.push(s);await s.attached('A');const target=s.target('A');
 s.held.add('session/subscribe');s.handlers.set('session/checkpoint',()=>({type:'checkpoint',checkpoint:checkpoint(snapshot(),'2')}));
 const work=s.client.refresh('A'),refused=expect(work).rejects.toThrow(/Subscription recovery required/);
 const subscribing=await s.waitFor('session/subscribe',2),before=s.client.getSnapshot().views.A;
 s.socket.success(subscribing,{type:'subscribed',target,registration_id:'pending-unavailable',after_cursor:'2'});
 s.socket.deliver({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target,registration_id:'pending-unavailable',message:'pending source fault'}});
 expect(s.client.getSnapshot().views.A.observationState).toBe('subscribePending');await refused;
 const failed=s.client.getSnapshot().views.A;
 expect(failed.observationState).toBe('unavailable');expect(failed.snapshot).toBe(before.snapshot);expect(failed.checkpoint).toBe(before.checkpoint);expect(failed.cursor).toBe('2');
 expect(s.client.isAttachmentControlCurrent('A',before.attachmentObservation)).toBe(false);
 await Promise.resolve();expect(s.requests.filter(row=>row.request.method==='session/checkpoint')).toHaveLength(2);expect(s.requests.filter(row=>row.request.method==='session/subscribe')).toHaveLength(2);expect(s.requests.filter(row=>row.request.method==='turn/start')).toHaveLength(0);
});
