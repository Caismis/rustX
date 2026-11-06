import assert from 'node:assert/strict';
import { it } from 'node:test';
import { harness, nextRequest } from './support/app-server-harness.ts';
import { paramsOf } from './support/app-server-peer.ts';

const node = 'node_00000000-0000-7000-8000-000000000001';
it('inspects an exact native schedule page with lossless ordinals and a usable next command', async () => {
  const h = await harness();
  const result = h.dispatcher.submit(`/schedules ${node} 9007199254740993`);
  const request = await nextRequest(h,'schedule/list');
  assert.deepEqual(paramsOf(request, 'schedule/list'),{session_id:h.session.sessionId,node_id:node,after_ordinal:'9007199254740993',limit:20});
  h.transport.respond(request.id,{type:'schedules',schedules:[{
    id:{conversation_id:h.target.conversation_id,ordinal:'9007199254740994'},
    binding:{session_id:h.session.sessionId,node_id:node,conversation_id:h.target.conversation_id},
    revision:'2',title:'Native reminder',target:{type:'conversation_inbound'},spec:{type:'interval',anchor_utc:'2026-10-24T10:00:00Z',every_seconds:600},state:'completed',
    next_due_utc:null,last_trigger:null,completed_at:'2026-10-24T10:00:00Z',completion_reason:'time_range_exhausted',last_failure:null,
  }],next_ordinal:'9007199254740995'});
  const output = JSON.stringify(await result);
  assert.match(output,/Native reminder/);
  assert.match(output,/No future occurrence/);
  assert.ok(output.includes(`/schedules ${node} 9007199254740995`));
});

it('discards a schedule page when its displayed Session is replaced', async () => {
  const h = await harness();
  const result = h.dispatcher.submit(`/schedules ${node}`);
  const request = await nextRequest(h,'schedule/list');
  h.dispatcher.setSession(undefined);
  h.transport.respond(request.id,{type:'schedules',schedules:[],next_ordinal:null});
  assert.deepEqual(await result,{kind:'none'});
});

it('preserves strict native uint8 bounds on Scheduler request budgets', async () => {
  const { decodeProtocolMessage } = await import('../src/protocol/decoder.ts');
  const record = (used: unknown) => ({ jsonrpc:'2.0', id: 'budget', result: {
    type:'schedule_command', result:{ status:'applied', receipt:{
      id:{conversation_id:'conv_00000000-0000-7000-8000-000000000001',ordinal:'1'}, revision:'1',
      spec:{type:'at',at_utc:'2026-10-24T10:00:00Z'},state:'completed',next_due_utc:null,
      request_budget:{ human_message_id:'human_fixture', original_attempt_id:'attempt_fixture', mutations_used:used, mutation_limit:8 },
    } },
  } });
  assert.ok(decodeProtocolMessage(record(8)));
  for (const used of [-1, 256, 1.5, '8']) assert.equal(decodeProtocolMessage(record(used)), undefined);
});

it('reads exact scheduled recovery with a lossless effect cursor and no activation',async()=>{
  const h=await harness(),startId='swf_'+'a'.repeat(64);
  const result=h.dispatcher.submit(`/workflow-recovery ${startId} ${node} 9007199254740993`);
  const request=await nextRequest(h,'workflow/scheduled/getRecovery');
  assert.deepEqual(paramsOf(request,'workflow/scheduled/getRecovery'),{session_id:h.session.sessionId,node_id:node,start_id:startId,after_effect_ordinal:'9007199254740993',limit:20});
  const run={conversation_id:h.target.conversation_id,origin:{type:'scheduled' as const,start_id:startId},invocation:'1'},time='2026-10-24T10:00:00Z';
  h.transport.respond(request.id,{type:'scheduled_workflow_recovery',recovery:{start:{id:startId,run_id:run,trigger_id:{schedule_id:{conversation_id:h.target.conversation_id,ordinal:'1'},admission_revision:'1',occurrence_utc:time},binding:{session_id:h.session.sessionId,node_id:node,conversation_id:h.target.conversation_id},accepted_at:time,state:{type:'claimed',owner:'native-owner',claimed_at:time,effects_started:true},native_quiescence_proven:false,recovery_blocked:'native physical proof unavailable'},effects:[],capacity_blocked:true,deletion_blocked:true,next_effect_ordinal:'9007199254740994'}});
  const output=JSON.stringify(await result);assert.match(output,/native physical proof unavailable/);assert.match(output,/9007199254740994/);
});
