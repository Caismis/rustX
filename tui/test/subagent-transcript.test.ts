import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SubagentTranscript } from '../src/app-server/subagent-transcript.ts';
import { emptyPresentationState } from '../src/presentation/projection.ts';
import { checkpoint, snapshot, subagent, userMessage } from './support/fixtures.ts';
import type { AgentObservation, AgentObservationCursor, RuntimeClientTranscriptPage } from '../../protocol/app-server/v57.ts';
import { harness, nextRequest } from './support/app-server-harness.ts';
const page = (id: string, next?: string): RuntimeClientTranscriptPage => ({ entries: [{ cursor: id, item: { type: 'message', message: userMessage(id, id) } }], next_cursor: next });
function controlled(id = 'A') {
  const agent = subagent('reader', 'definition', 'active', { agent_id: id });
  const state = { ...emptyPresentationState(null), agents: [agent] };
  const reads: { after?: AgentObservationCursor; signal: AbortSignal; resolve(value: AgentObservation): void; reject(error: Error): void }[] = [];
  const pages: { before?: string; resolve(value: RuntimeClientTranscriptPage): void }[] = [];
  const waiting = new Map<number, () => void>();
  const reader = new SubagentTranscript({ state,checkpoint:undefined,onState:()=>()=>{},agentStatus:async()=>agent,
    observeAgent: (_id, after, signal) => new Promise((resolve, reject) => { reads.push({ after, signal, resolve, reject }); waiting.get(reads.length - 1)?.(); }),
    agentTranscriptPage: (_id, before) => new Promise(resolve => { pages.push({ before, resolve }); }),
    agentTraceInspection:async()=>{throw new Error('No Child Trace selection');},agentInspectionPage:async()=>{throw new Error('No Child Trace message page');},readAgentTraceField:async()=>{throw new Error('No Child Trace field');},readAttemptIdentity:async()=>{throw new Error('No Child owner identity');},readChildJournalField:async()=>{throw new Error('No Child result');},readContent: async () => { throw new Error('Unexpected content read'); },
  }, id);
  const waitRead = async (index: number) => { if (!reads[index]) await new Promise<void>(resolve => waiting.set(index, resolve)); };
  const observed = (history: RuntimeClientTranscriptPage, source: 'live' | 'durable_only' = 'live', cursor = '0'): Extract<AgentObservation,{type:'checkpoint'}> => ({ type: 'checkpoint', agent_id: id, conversation_id: agent.child_conversation_id, activation_id: agent.activation_id, source,
    cursor: source === 'live' ? { agent_id: id, conversation_id: agent.child_conversation_id, activation_id: agent.activation_id, cursor } : null,
    checkpoint: { ...checkpoint({ ...snapshot(), conversation_id: agent.child_conversation_id, transcript: history }, cursor), read_cut: { ...checkpoint({ ...snapshot(), conversation_id: agent.child_conversation_id, transcript: history }, cursor).read_cut, activation_id: agent.activation_id } },
  });
  return { reader, agent, state, reads, pages, waitRead, observed };
}
test('T18/T20 view disposal cancels observation only and fences late Child success or failure', async () => {
  for (const failure of [false, true]) {
    const a = controlled('A'), b = controlled('B'); await a.reader.newest();
    a.reader.dispose(); assert.equal(a.reads[0]!.signal.aborted, true);
    await b.reader.newest();
    b.reads[0]!.resolve(b.observed(page('20'))); await b.waitRead(1);
    if (failure) a.reads[0]!.reject(new Error('Old source failed')); else a.reads[0]!.resolve(a.observed(page('10')));
    assert.equal(a.reader.page, undefined); assert.deepEqual(b.reader.page, page('20')); assert.equal(b.reader.error, undefined);
    b.reader.dispose(); b.reads[1]!.resolve({ type: 'events', after: b.reads[1]!.after!, cursor: b.reads[1]!.after!, events: [] });
  }
});
test('T05/T18 native events follow Applied continuously and DurableOnly stops without execution settlement', async () => {
  const live = controlled(); await live.reader.newest(); live.reads[0]!.resolve(live.observed(page('10'))); await live.waitRead(1);
  assert.equal(live.reads[1]!.after?.cursor, '0');
  const after = live.reads[1]!.after!;
  const next = live.observed(page('20'), 'live', '1').checkpoint;
  live.reads[1]!.resolve({ type: 'events', after, cursor: { ...after, cursor: '1' }, events: [{ cursor: '1', event: { type:'projection_transition', metadata:{read_cut:next.read_cut,content:[],collections:[]},events:[
    { type: 'message_committed', message: userMessage('20', '20'), transcript_cursor: '20' },
    // Native C commitment advances its display head; the captured T window is
    // replaced only by the corresponding source-owned read-domain fact.
    { type: 'read_domains_updated', transcript: next.snapshot.transcript, trace: next.snapshot.trace, todos: next.snapshot.todos, occupancy: next.snapshot.context?.occupancy??null },
  ] } }] }); await live.waitRead(2);
  assert.equal(live.reads[2]!.after?.cursor, '1'); assert.equal(live.reader.page?.entries?.at(-1)?.cursor, '20');
  live.reader.dispose(); live.reads[2]!.resolve({ type: 'events', after: live.reads[2]!.after!, cursor: live.reads[2]!.after!, events: [] });
  const durable = controlled(); await durable.reader.newest();
  const changed = new Promise<void>(resolve => { durable.reader.onChange = resolve; }); durable.reads[0]!.resolve(durable.observed(page('30'), 'durable_only')); await changed;
  assert.equal(durable.reader.phase, 'durableOnly'); assert.equal(durable.reads.length, 1); assert.equal(durable.reader.state?.attempt, undefined); durable.reader.dispose();
});
test('T20 historical paging replaces one bounded page and a late page cannot overwrite newer ownership', async () => {
  const h = controlled(); await h.reader.newest(); h.reads[0]!.resolve(h.observed(page('30', '30'))); await h.waitRead(1);
  const older = h.reader.older(); assert.equal(h.reads[1]!.signal.aborted, true); assert.equal(h.pages[0]!.before, '30');
  h.reader.dispose(); h.pages[0]!.resolve(page('20', '20')); await older; assert.equal(h.reader.page, undefined);
  h.reads[1]!.resolve({ type: 'events', after: h.reads[1]!.after!, cursor: h.reads[1]!.after!, events: [] });
});
test('T18 source and activation mismatch is observation unavailability, never an execution outcome', async () => {
  const h = controlled(); await h.reader.newest();
  const changed = new Promise<void>(resolve => { h.reader.onChange = resolve; });
  h.reads[0]!.resolve({ ...h.observed(page('10')), activation_id: 'obsolete' } as AgentObservation); await changed;
  assert.match(h.reader.error!, /activation was replaced/); assert.equal(h.reader.page, undefined); assert.equal(h.state.agents[0]!.state, 'active'); h.reader.dispose();
});
test('parent detach fences historical continuation and reads name the exact parent and Agent only', async () => {
  const h = await harness(); const reading = h.session.agentTranscriptPage('A', '12'); const read = await nextRequest(h, 'agent/transcript', 0);
  assert.deepEqual(read.params, { target: h.target, agent_id: 'A', at: { type: 'older', before: '12', cut: null }, limit: 32 });
  const detached = h.session.detach(); const detach = await nextRequest(h, 'session/detach', 0); h.transport.respond(detach.id, { type: 'detached' }); await detached;
  h.transport.respond(read.id, { type: 'transcript_window', window: { cut: { conversation_id: snapshot().conversation_id, journal: '20', transcript: '20', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, page: page('10') } });
  assert.equal(await reading, undefined);
  for (const method of ['turn/start', 'turn/steer', 'agent/interrupt', 'interaction/respond'] as const) assert.equal(h.transport.transportCount(method), 0); h.client.close();
});

test('T11/T18 off-head Child source refresh uses bounded status and rejects an old Activation cursor', async () => {
  const state=emptyPresentationState(null);
  let agent=subagent('older-reader','native-definition','inactive',{agent_id:'off-head'});
  let rootCheckpoint=checkpoint(snapshot());
  let notify!:()=>void;
  let statusReads=0;
  const reads:{after?:AgentObservationCursor;resolve:(value:AgentObservation)=>void}[]=[];
  const waiting=new Map<number,()=>void>();
  const reader=new SubagentTranscript({state,get checkpoint(){return rootCheckpoint;},onState:listener=>{notify=()=>listener(state);return()=>{};},
    agentStatus:async()=>{statusReads++;return agent;},
    observeAgent:(_id,after,signal)=>new Promise((resolve,reject)=>{signal.addEventListener('abort',()=>reject(new Error('Observation cancelled')),{once:true});reads.push({after,resolve});waiting.get(reads.length-1)?.();}),
    agentTranscriptPage:async()=>{throw new Error('No history request');},agentTraceInspection:async()=>{throw new Error('No Child Trace selection');},agentInspectionPage:async()=>{throw new Error('No Child Trace message page');},readAgentTraceField:async()=>{throw new Error('No Child Trace field');},readAttemptIdentity:async()=>{throw new Error('No Child owner identity');},readChildJournalField:async()=>{throw new Error('No Child result');},readContent:async()=>{throw new Error('No content request');},
  },'off-head');
  const waitRead=async(index:number)=>{if(!reads[index])await new Promise<void>(resolve=>waiting.set(index,resolve));};
  const observed=(source:'live'|'durable_only'):AgentObservation=>({type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source,cursor:source==='live'?{agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,cursor:'0'}:null,checkpoint:{...checkpoint({...snapshot(),conversation_id:agent.child_conversation_id}),read_cut:{...checkpoint({...snapshot(),conversation_id:agent.child_conversation_id}).read_cut,activation_id:agent.activation_id}}});
  await reader.newest();assert.equal(statusReads,1);assert.equal(reads[0]!.after,undefined);
  const durableApplied=new Promise<void>(resolve=>{reader.onChange=resolve;});reads[0]!.resolve(observed('durable_only'));await durableApplied;
  assert.equal(reader.phase,'durableOnly');assert.equal(state.agents.length,0);
  agent={...agent,state:'active',activation_id:'activation-two',current_activation:'activation-two'};
  rootCheckpoint={...rootCheckpoint,read_cut:{...rootCheckpoint.read_cut,domain_revisions:{agents:'1'}}};notify();
  await waitRead(1);assert.equal(statusReads,2);assert.equal(reads[1]!.after,undefined);assert.equal(reader.agent?.activation_id,'activation-two');
  reads[1]!.resolve(observed('live'));await waitRead(2);
  assert.equal(reads[2]!.after?.activation_id,'activation-two');assert.equal(state.agents.length,0);assert.equal(state.cursor,'0');
  reader.dispose();
});

test('T20 a late off-head status read cannot reopen a disposed Child view', async()=>{
  let resolve!: (value:ReturnType<typeof subagent>)=>void;
  let observed=0;
  let entered!:()=>void;const admitted=new Promise<void>(done=>{entered=done;});
  const reader=new SubagentTranscript({state:emptyPresentationState(null),checkpoint:undefined,onState:()=>()=>{},agentStatus:()=>new Promise(done=>{resolve=done;entered();}),observeAgent:async()=>{observed++;throw new Error('No admitted observation');},agentTranscriptPage:async()=>{throw new Error('No history');},agentTraceInspection:async()=>{throw new Error('No Child Trace selection');},agentInspectionPage:async()=>{throw new Error('No Child Trace message page');},readAgentTraceField:async()=>{throw new Error('No Child Trace field');},readAttemptIdentity:async()=>{throw new Error('No Child owner identity');},readChildJournalField:async()=>{throw new Error('No Child result');},readContent:async()=>{throw new Error('No content');}},'off-head');
  const opening=reader.newest();await admitted;reader.dispose();resolve(subagent('old','definition','active',{agent_id:'off-head'}));await opening;
  assert.equal(observed,0);assert.equal(reader.page,undefined);
});

test('T20 repeated Home coalesces setup and waits for exact observation cancellation before replacement',async()=>{
 const h=controlled();await h.reader.newest();h.reads[0]!.resolve(h.observed(page('10')));await h.waitRead(1);
 const first=h.reader.newest(),second=h.reader.newest();assert.equal(first,second);assert.equal(h.reads[1]!.signal.aborted,true);assert.equal(h.reads.length,2);
 h.reads[1]!.resolve({type:'events',after:h.reads[1]!.after!,cursor:h.reads[1]!.after!,events:[]});await Promise.all([first,second]);
 assert.equal(h.reads.length,3);assert.equal(h.reads[2]!.after?.cursor,'0');
 h.reader.dispose();h.reads[2]!.resolve({type:'events',after:h.reads[2]!.after!,cursor:h.reads[2]!.after!,events:[]});
});

test('T08/T19 Child Tool result reads carry its exact native cut and activation view ownership',async()=>{
 const {attemptView,foreground}=await import('./support/fixtures.ts');
 const agent=subagent('reader','definition','active');
 const h=await harness(snapshot({agents:[agent]}));
 const reader=new SubagentTranscript(h.session,agent.agent_id);await reader.newest();
 const observed=await nextRequest(h,'agent/observe');
 const bytes=Buffer.from('"🙂水"'),reference={sequence:17,path:['event','result'],encoded_bytes:bytes.length};
 const native=checkpoint(snapshot({conversation_id:agent.child_conversation_id,attempt:attemptView({foreground:[{...foreground('call','tool','name',{type:'running',arguments:'{}'}),result_source:reference}]})}));native.read_cut.activation_id=agent.activation_id;
 h.transport.respond(observed.id,{type:'agent_observe',observation:{type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source:'live',cursor:{agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,cursor:'0'},checkpoint:native}});
 await nextRequest(h,'agent/observe',1);
 assert.equal(reader.deferredCount,1);
 const loading=reader.loadContent();const read=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(read.params,{target:h.target,agent_id:agent.agent_id,cut:native.read_cut,domain:'journal.17',item_offset:0,path:reference.path,offset:0,max_bytes:65536});
 h.transport.respond(read.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut:native.read_cut,domain:'journal.17',item_offset:0,path:reference.path,offset:0,bytes_base64:bytes.toString('base64'),next_offset:null,readable_end:bytes.length}});
 await loading;assert.equal(reader.content?.text,bytes.toString('utf8'));assert.equal(h.session.state.cursor,'0');
 reader.dispose();h.client.close();
});


test('T19 Child binary content remains exact Base64 instead of entering a UTF8 decoder',async()=>{
 const agent=subagent('reader','definition','active');
 const h=await harness(snapshot({agents:[agent]}));
 const reader=new SubagentTranscript(h.session,agent.agent_id);await reader.newest();
 const observe=await nextRequest(h,'agent/observe');
 const bytes=Buffer.from([0xff,0,0xc0,0x81]);
 const reference={conversation_id:agent.child_conversation_id,source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'binary_bytes' as const,version:1,content_revision:1,readable_end:bytes.length};
 const native=checkpoint(snapshot({conversation_id:agent.child_conversation_id}));native.read_cut.activation_id=agent.activation_id;
 native.content=[{path:['binary'],source_path:[],content_ref:reference,deferred:true}];
 const applied=new Promise<void>(resolve=>{reader.onChange=resolve;});
 h.transport.respond(observe.id,{type:'agent_observe',observation:{type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source:'durable_only',cursor:null,checkpoint:native}});
 await applied;
 const loading=reader.loadContent();const read=await nextRequest(h,'conversation/contentRead');
 assert.equal((read.params as {agent_id:string}).agent_id,agent.agent_id);
 h.transport.respond(read.id,{type:'content_chunk',chunk:{content_ref:reference,offset:0,data_base64:bytes.toString('base64'),end_offset:bytes.length,readable_end:bytes.length,next_offset:null,complete:true}});
 await loading;
 assert.equal(reader.content?.text,`Base64: ${bytes.toString('base64')}`);
 assert.equal(reader.phase,'durableOnly');assert.equal(reader.state?.cursor,'0');
 reader.dispose();h.client.close();
});


test('T08 T19 DurableOnly Child failure bytes stay normalized and do not manufacture execution transitions', async () => {
  const {attemptView}=await import('./support/fixtures.ts');
  const agent=subagent('reader','definition','inactive');
  const h=await harness(snapshot({agents:[agent]}));
  const reader=new SubagentTranscript(h.session,agent.agent_id);
  await reader.newest();
  const observe=await nextRequest(h,'agent/observe');
  const bytes=Buffer.from('{"type":"model","message":"🙂水","kind":"provider_error","retry_after_ms":null}');
  const reference={sequence:31,path:['normalized_failure'],encoded_bytes:bytes.length};
  const native=checkpoint(snapshot({conversation_id:agent.child_conversation_id,attempt:attemptView({phase:{type:'settled',outcome:{type:'failed',error:{type:'model',kind:'provider_error',message:'deferred',retry_after_ms:null},error_deferred:true,error_source:reference}}})}));
  native.read_cut.activation_id=agent.activation_id;
  const installed=new Promise<void>(resolve=>{reader.onChange=resolve;});
  h.transport.respond(observe.id,{type:'agent_observe',observation:{type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source:'durable_only',cursor:null,checkpoint:native}});
  await installed;
  assert.equal(reader.phase,'durableOnly');
  assert.equal(reader.deferredJournalFields[0]?.label,'Attempt failure');
  const loading=reader.loadContent();
  const read=await nextRequest(h,'conversation/checkpointValueRead');
  assert.deepEqual(read.params,{target:h.target,agent_id:agent.agent_id,cut:native.read_cut,domain:'journal.31',item_offset:0,path:reference.path,offset:0,max_bytes:65536});
  h.transport.respond(read.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut:native.read_cut,domain:'journal.31',item_offset:0,path:reference.path,offset:0,bytes_base64:bytes.toString('base64'),next_offset:null,readable_end:bytes.length}});
  await loading;
  assert.equal(reader.content?.text,bytes.toString('utf8'));
  assert.equal(reader.phase,'durableOnly');
  assert.equal(h.session.state.agents[0]?.state,'inactive');
  assert.equal(h.session.state.cursor,'0');
  assert.equal(h.transport.log.matching('agent/observe').length,1);
  reader.dispose();h.client.close();
});


test('T08 T19 Child UTF8 ranges preserve a split literal leading U+FEFF', async () => {
 const agent=subagent('reader','definition','active');
 const h=await harness(snapshot({agents:[agent]}));
 const reader=new SubagentTranscript(h.session,agent.agent_id);await reader.newest();
 const observe=await nextRequest(h,'agent/observe');
 const original='\uFEFF🙂水', bytes=Buffer.from(original);
 const reference={conversation_id:agent.child_conversation_id,source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'utf8_text' as const,version:1,content_revision:1,readable_end:bytes.length};
 const native=checkpoint(snapshot({conversation_id:agent.child_conversation_id}));native.read_cut.activation_id=agent.activation_id;
 native.content=[{path:['text'],source_path:[],content_ref:reference,deferred:true}];
 const applied=new Promise<void>(resolve=>{reader.onChange=resolve;});
 h.transport.respond(observe.id,{type:'agent_observe',observation:{type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source:'durable_only',cursor:null,checkpoint:native}});
 await applied;
 const first=reader.loadContent();const read=await nextRequest(h,'conversation/contentRead');
 h.transport.respond(read.id,{type:'content_chunk',chunk:{content_ref:reference,offset:0,data_base64:bytes.subarray(0,1).toString('base64'),end_offset:1,readable_end:bytes.length,next_offset:1,complete:false}});
 await first;assert.equal(reader.content?.text,'');assert.equal(reader.content?.next,1);
 const second=reader.loadContent(0,1);const next=await nextRequest(h,'conversation/contentRead');
 h.transport.respond(next.id,{type:'content_chunk',chunk:{content_ref:reference,offset:1,data_base64:bytes.subarray(1).toString('base64'),end_offset:bytes.length,readable_end:bytes.length,next_offset:null,complete:true}});
 await second;assert.equal(reader.content?.text,original);
 assert.equal(reader.phase,'durableOnly');assert.equal(reader.state?.cursor,'0');
 reader.dispose();h.client.close();
});
