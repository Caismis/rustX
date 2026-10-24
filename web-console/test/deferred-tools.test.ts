import { expect, it } from 'vitest';
import type { ForegroundToolExecution, RuntimeClientMessageDescriptor, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../protocol/app-server/v57';
import { deferredToolDescriptors } from '../src/bindings/deferred-tools';

const locator = {type:'by_ledger_position' as const,position:73};
const tool: ForegroundToolExecution = {
  occurrence:{locator,block_index:9},block_index:9,name:'bash',assembly_complete:true,
  committed_result_deferred:true,
  state:{type:'running',arguments:'{"command":"original"}'},
  committed_result:{status:{type:'success'},content:[],duration_ms:'7'},
};
const descriptor = (tools=[tool]):RuntimeClientMessageDescriptor => ({locator,role:'assistant',kind:'message',
  block_count:'9007199254740993',tool_call_count:'1',tool_calls:tools,turn_process:null,completed_response:null,content:[],collections:[]});
const page = (value:unknown):RuntimeClientTranscriptPage => ({entries:[],deferred_entries:[{
  offset:'72',identity:'native-ledger:73',value,content:[],deferred:true,
}]});

it('T15/T19 a deferred canonical Message preserves native Tool occurrences without fabricating execution settlement',()=>{
  const source = descriptor();
  const rows = deferredToolDescriptors([],page(source),[],true);
  expect(rows).toEqual([source]);
  expect(rows[0]!.identity).toBeUndefined();
  expect(rows[0]!.tool_calls[0]!.occurrence).toBe(tool.occurrence);
  expect(rows[0]!.tool_calls[0]!.state).toBe(tool.state);
  expect(rows[0]!.tool_calls[0]!.committed_result).toBe(tool.committed_result);
});

it('T14 a frozen historical cut retains its own J evidence while a live head supersedes an older byte cache',()=>{
  const frozen = descriptor();
  const terminal:ForegroundToolExecution = {...tool,state:{type:'settled',arguments:tool.state.arguments,
    result:{status:{type:'failed',error:'native failure'},content:[],duration_ms:'8'}}};
  const current = descriptor([terminal]);
  const cached = page(frozen);
  expect(deferredToolDescriptors([current],cached,[],true)).toEqual([frozen]);
  expect(deferredToolDescriptors([current],cached,[],false)).toEqual([current]);
  expect(cached.deferred_entries![0]!.value).toBe(frozen);
  expect(frozen.tool_calls[0]!.state).toBe(tool.state);
});

it('T15 loaded native occurrences suppress only their exact canonical locator',()=>{
  const loaded:RuntimeClientTranscriptEntry = {cursor:'73',tool_calls:[tool],item:{type:'message',message:{
    role:'assistant',id:'short-original-id',content:[],
  }}};
  const same = descriptor(), other = {...descriptor(),locator:{type:'by_ledger_position' as const,position:74}};
  expect(deferredToolDescriptors([same,other],page(same),[loaded],false)).toEqual([other]);
  const identity = {...descriptor(),identity:'short-original-id'};
  expect(deferredToolDescriptors([],page(identity),[{...loaded,tool_calls:[]}],true)).toEqual([]);
});

it('T15 canonical body objects and malformed descriptors cannot stand in for a native display head',()=>{
  for(const value of [null,{role:'assistant',id:'m',content:[]},
    {...descriptor(),locator:{type:'by_ledger_position',position:9007199254740992}},
    {...descriptor(),tool_calls:Array.from({length:33},()=>tool)},
  ])expect(deferredToolDescriptors([],page(value),[],true)).toEqual([]);
});

it('T14 deferred descriptors retain native same-attempt process and completion facts independently of body bytes',()=>{
  const process={attempt_id_source:null,conversation_id:'c',attempt_id:'original-attempt',outcome:'completed' as const,control_cursor:'74',message_count:2,tool_call_count:1,
    final_message_id:null,final_message_locator:locator,final_message_id_deferred:false,final_message_id_source:null};
  const response={closing_message_id:null,closing_locator:locator,deferred_fields:[],origin:{attempt_id_source:null,conversation_id:'c',attempt_id:'original-attempt'},completed_at:'2026-10-14T00:00:00Z'};
  const source={...descriptor(),turn_process:process,completed_response:response};
  const rows=deferredToolDescriptors([],page(source),[],true);
  expect(rows[0]!.turn_process).toBe(process);
  expect(rows[0]!.completed_response).toBe(response);
  expect(rows[0]!.tool_calls[0]!.state.type).toBe('running');
  expect(source.content).toEqual([]);
});
