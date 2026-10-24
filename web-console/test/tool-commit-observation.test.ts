import { expect, it } from 'vitest';
import { checkpointSnapshot, contentBindings, observationCheckpoint, foldRuntimeEvent as fold } from '../../protocol/app-server/projection';
import type { ForegroundToolExecution, RuntimeClientEvent, ToolExecutionResult } from '../../protocol/app-server/v57';
import { toolCard } from '../src/bindings/tools';
import { checkpoint, snapshot } from './fixture';

const occurrence = {locator:{type:'by_ledger_position' as const,position:7},block_index:2};
const tool: ForegroundToolExecution = {message_id:'message',occurrence,block_index:2,call_id:'call',tool_id:'tool-bash',name:'bash',assembly_complete:true,committed_result_deferred:false,state:{type:'running',arguments:'{"command":"original"}'}};
const canonical: ToolExecutionResult = {status:{type:'success'},content:[{type:'text',text:'canonical body'}],duration_ms:"70",exit_code:0};
const terminal: ToolExecutionResult = {status:{type:'failed',error:'native execution failure'},content:[],duration_ms:"23",exit_code:3};
const committed: RuntimeClientEvent = {type:'tool_result_committed',attempt_id:'attempt',occurrence,result:canonical,result_metadata_ref:null,result_deferred:false};

it('T14/T19 canonical Tool commitment does not fabricate execution settlement or overwrite Journal evidence', () => {
  const state = {...snapshot(),attempt:{attempt_id:'attempt',turn:1,phase:{type:'running' as const},foreground:[{...tool,result_source:{sequence:4,path:['event','result'],encoded_bytes:19}}]}};
  const after = fold(state,committed), next = after.attempt!.foreground![0]!;
  expect(next.state).toBe(state.attempt.foreground[0]!.state);
  expect(next.result_source).toEqual(state.attempt.foreground[0]!.result_source);
  expect(next.committed_result).toBe(canonical);
  expect(toolCard(next)).toMatchObject({state:'running',output:'canonical body'});
});

it('T14/T19 a late canonical result changes the displayed body without retracting terminal Journal status', () => {
  const state = {...snapshot(),attempt:{attempt_id:'attempt',turn:1,phase:{type:'running' as const},foreground:[tool]}};
  const settled = fold(state,{type:'tool_execution_settled',attempt_id:'attempt',occurrence,result:terminal});
  const after = fold(settled,committed), next = after.attempt!.foreground![0]!;
  expect(next.state).toBe(settled.attempt!.foreground![0]!.state);
  expect(next.state).toMatchObject({type:'settled',result:terminal});
  expect(next.committed_result).toBe(canonical);
  expect(toolCard(next)).toMatchObject({state:'failure',exitCode:3,output:'native execution failure\ncanonical body'});
});

it('T14 a canonical occurrence never aliases another block or creates a missing Tool slot', () => {
  const state = {...snapshot(),attempt:{attempt_id:'attempt',turn:1,phase:{type:'running' as const},foreground:[tool]}};
  const mismatched = {...committed,occurrence:{...occurrence,block_index:3}};
  expect(fold(state,mismatched).attempt!.foreground![0]!.committed_result).toBeUndefined();
  const missing = {...state,attempt:{...state.attempt,foreground:[]}};
  expect(fold(missing,committed).attempt!.foreground).toEqual([]);
});

it('T14 detached progress cannot claim a foreground occurrence with the same Tool identity', () => {
  const state = {...snapshot(),attempt:{attempt_id:'attempt',turn:1,phase:{type:'running' as const},foreground:[tool]}};
  expect(fold(state,{type:'tool_execution_progress',attempt_id:'attempt',occurrence,tool_call_id:'call',execution_id:'background-job',progress:{completed:1,total:2}})).toBe(state);
  const foreground = fold(state,{type:'tool_execution_progress',attempt_id:'attempt',occurrence,tool_call_id:null,execution_id:null,progress:{completed:1,total:2}});
  expect(foreground.attempt!.foreground![0]!.state).toMatchObject({type:'running',progress:{completed:1,total:2}});
});

it('T14 a new native publication retires the prior turn foreground without changing execution settlement', () => {
  const state = {...snapshot(),attempt:{attempt_id:'attempt',turn:2,phase:{type:'running' as const},foreground:[tool]}};
  const next = fold(state,{type:'assistant_message_started',attempt_id:'attempt',message_id:'next-publication'});
  expect(next.attempt!.foreground).toEqual([]);
  expect(next.attempt!.phase).toBe(state.attempt.phase);
  expect(next.attempt!.in_flight).toEqual({message_id:'next-publication',blocks:[]});
  expect(fold(next,committed).attempt!.foreground).toEqual([]);
});

const auditIdentity = {stream_id:'stream-current',attempt_id:'attempt',turn_id:'turn',request_id:'request',message_id:'message-current'};
const publication = () => {
  const initial = checkpoint({...snapshot(),attempt:{attempt_id:'attempt',turn:2,phase:{type:'running'},in_flight:{message_id:'message-current',blocks:[{type:'text',block_index:0,text:'released'}]},foreground:[]}});
  initial.content=[{path:['attempt','in_flight','blocks','0','text'],deferred:false,content_ref:{conversation_id:initial.snapshot.conversation_id,source:{type:'live_publication_prefix',component:'payload',stream_id:auditIdentity.stream_id,message_id:auditIdentity.message_id,attempt_id:auditIdentity.attempt_id,turn_id:auditIdentity.turn_id,publication_request_id:auditIdentity.request_id,block_index:0},representation:'utf8_text',version:1,content_revision:1,readable_end:8}}];
  return checkpointSnapshot(initial);
};

it('T14/T19 deferred native audit records positive settlement while leaving a later publication and execution untouched', () => {
  const state = publication();
  const settlement = {...auditIdentity,stream_id:'stream-old',message_id:'message-old',kind:'incomplete' as const,settled_at:'2026-10-13T00:00:00Z',block_count:'9007199254740993'};
  const read_cut={...checkpoint(state).read_cut.source_cut!,read:{conversation_id:state.conversation_id,journal:'0',transcript:'1',mutation_revision:'0',ledger_through:'0',surface_revision:'0',transcript_total:'1',pending_through:'0',pending_total:'0'},ledger_through:'0',transcript_total:'1',surface_revision:'0',inherited_through:'0'};
  const settlement_head=[{settlement,transcript_cursor:'1',read_cut}];
  const next=fold(state,{type:'assistant_publication_deferred_settled',attempt_id:'attempt',settlement,settlement_head,transcript_cursor:'1',read_cut,content:[],collections:[]});
  expect(next.publication_settlements).toBe(settlement_head);
  expect(next.attempt).toBe(state.attempt);
  expect(contentBindings(next)).toEqual(contentBindings(state));
  expect(next.messages).toBe(state.messages);
});

it('T14/T19 exact audit settlement retires only its publication bytes and preserves canonical and execution distinctions', () => {
  const state=publication();
  const audit={...auditIdentity,kind:'unaccepted' as const,content:[],settled_at:'2026-10-13T00:00:00Z'};
  const next=fold(state,{type:'assistant_publication_settled',attempt_id:'attempt',audit,transcript_cursor:'1',settlement_head:[]});
  expect(next.attempt!.in_flight).toBeUndefined();
  expect(next.attempt!.phase).toBe(state.attempt!.phase);
  expect(next.messages).toBe(state.messages);
  expect(contentBindings(next)).toEqual([]);
});

it('T14/T19 an audit with another stream cannot retire a live prefix through a shared message identity', () => {
  const state=publication();
  const next=fold(state,{type:'assistant_publication_settled',attempt_id:'attempt',audit:{...auditIdentity,stream_id:'another-stream',kind:'incomplete',content:[],settled_at:'2026-10-13T00:00:00Z'},transcript_cursor:'1',settlement_head:[]});
  expect(next.attempt).toBe(state.attempt);
  expect(contentBindings(next)).toEqual(contentBindings(state));
});

it.each(['assistant_publication_settled','assistant_publication_deferred_settled'] as const)('T14/T19 %s cannot retire a retry at H=0 through its reused Message', type => {
  const initial=checkpoint({...snapshot(),attempt:{attempt_id:'attempt',turn:2,phase:{type:'running'},in_flight:{message_id:auditIdentity.message_id,blocks:[]},foreground:[]}});
  initial.read_cut.live_publication={source:{type:'live_publication_prefix',component:'payload',stream_id:auditIdentity.stream_id,message_id:auditIdentity.message_id,attempt_id:auditIdentity.attempt_id,turn_id:auditIdentity.turn_id,publication_request_id:auditIdentity.request_id,block_index:0},revision:0,block_total:0,tool_total:0};
  initial.content=[];
  const state=checkpointSnapshot(initial);
  const settlement={...auditIdentity,stream_id:'retired-stream',turn_id:'retired-turn',request_id:'retired-request',kind:'unaccepted' as const,settled_at:'2026-10-13T00:00:00Z',block_count:'0'};
  const read_cut=initial.read_cut.source_cut!;
  const settlement_head=[{settlement,transcript_cursor:'1',read_cut}];
  const event:RuntimeClientEvent=type==='assistant_publication_settled'
    ? {type,attempt_id:'attempt',audit:{...settlement,content:[]},transcript_cursor:'1',settlement_head}
    : {type,attempt_id:'attempt',settlement,transcript_cursor:'1',settlement_head,read_cut,content:[],collections:[]};
  const next=fold(state,event);
  expect(next.publication_settlements).toBe(settlement_head);
  expect(next.attempt).toBe(state.attempt);
  expect(observationCheckpoint(next,'1')!.read_cut.live_publication).toBe(initial.read_cut.live_publication);
  expect(contentBindings(next)).toEqual([]);
  const current={...settlement,...auditIdentity};
  const accepted=fold(next,{type:'assistant_publication_settled',attempt_id:'attempt',audit:{...current,content:[]},transcript_cursor:'2',settlement_head});
  expect(accepted.attempt!.in_flight).toBeUndefined();
  expect(accepted.attempt!.phase).toBe(state.attempt!.phase);
});

it('T14 a new native transition leaves the captured checkpoint metadata at C unchanged', () => {
  const initial=checkpoint(snapshot(),'4');
  const state=checkpointSnapshot(initial);
  const nextCut={...initial.read_cut,revision:"5"};
  const next=fold(state,{type:'projection_transition',events:[{type:'trace_changed'}],metadata:{read_cut:nextCut,content:[],collections:[]}});
  expect(observationCheckpoint(next,'5')!.read_cut).toBe(nextCut);
  expect(observationCheckpoint(state,'4')).toEqual(initial);
});

it('T14 canonical C for an earlier publication cannot retire the current provisional message', () => {
  const state=publication();
  const next=fold(state,{type:'message_committed',attempt_id:'attempt',message:{role:'assistant',id:'prior-canonical',content:[{type:'text',text:'canonical'}]}});
  expect(next.attempt).toBe(state.attempt);
  expect(contentBindings(next)).toEqual(contentBindings(state));
});

it('T11/T19 source, publication stream, and representation mismatch cannot advance a display transition', () => {
 const state=publication(),reference=contentBindings(state)[0]!.content_ref;
 const event:Extract<RuntimeClientEvent,{type:'block_content_advanced'}>={type:'block_content_advanced',position:0,attempt_id:'attempt',message_id:'message-current',block_index:0,kind:'text',base_length:'8',end_length:'9',content_revision:'2',inline_delta:'!',content_ref:{...reference,content_revision:2,readable_end:9}};
 const valid=fold(state,event);expect(valid.attempt!.in_flight!.blocks![0]).toMatchObject({text:'released!'});
 for(const changed of [
  {...event,kind:'unsupported'},
  {...event,content_ref:{...event.content_ref,representation:'raw_json_utf8' as const}},
  {...event,content_ref:{...event.content_ref,conversation_id:'another-conversation'}},
  {...event,content_ref:{...event.content_ref,source:{...reference.source,type:'live_publication_prefix' as const,component:'payload' as const,stream_id:'another-stream',message_id:'message-current',attempt_id:'attempt',turn_id:'turn',publication_request_id:'request',block_index:0}}},
 ])expect(()=>fold(state,changed)).toThrow();
 expect(state.attempt!.in_flight!.blocks![0]).toMatchObject({text:'released'});
});
