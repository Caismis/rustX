import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ForegroundToolExecution, RuntimeClientMessageDescriptor, RuntimeClientTranscriptPage } from '../../protocol/app-server/v52';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { isFinalTurnDescriptor } from '../src/bindings/turn-process';
import { JournalSourceProvider } from '../src/app/agent/JournalValue';
import { checkpoint, readCut, Server, snapshot } from './fixture';

afterEach(cleanup);
it('T11/T14 a historical deferred Tool row keeps its captured cut even when the same Message is present in the live tail',()=>{
  const locator = {type:'by_ledger_position' as const,position:7};
  const frozen:ForegroundToolExecution = {occurrence:{locator,block_index:0},message_id:'m',block_index:0,
    call_id:'original',name:'bash',assembly_complete:true,committed_result_deferred:true,
    state:{type:'running',arguments:'{}'},committed_result:{status:{type:'success'},content:[],duration_ms:'1'}};
  const descriptor:RuntimeClientMessageDescriptor = {identity:'m',locator,role:'assistant',kind:'message',
    block_count:'9007199254740993',tool_call_count:'1',tool_calls:[frozen],turn_process:null,completed_response:null,content:[],collections:[]};
  const page:RuntimeClientTranscriptPage = {entries:[],deferred_entries:[{offset:'0',identity:'m',value:descriptor,content:[],deferred:true}]};
  const terminal:ForegroundToolExecution = {...frozen,state:{type:'settled',arguments:'{}',result:{status:{type:'failed',error:'late native failure'},content:[],duration_ms:'2'}}};
  const live = {...snapshot(),messages:[{role:'assistant' as const,id:'m',content:[]}],
    attempt:{attempt_id:'a',turn:1,phase:{type:'running' as const},foreground:[terminal]},
    message_descriptors:[{...descriptor,tool_calls:[terminal]}]};
  const ui = render(<AgentTranscript snapshot={live} history={{epoch:1,page,window:{cut:readCut(),target_cursor:'7',page}}}/>);
  expect(ui.container.querySelector('[data-canonical-locator]')).not.toBeNull();
  expect(ui.container.querySelector('[data-tool-call-id="original"] [data-state]')?.getAttribute('data-state')).toBe('running');
  expect(ui.container.querySelector('[data-tool-result-committed]')?.textContent).toContain('success');
  expect(ui.queryByText('late native failure')).toBeNull();
});


it('T14 a deferred process seat displays only original native completion and exact final locator',()=>{
  const locator={type:'by_ledger_position' as const,position:17};
  const owner={attempt_id_source:null,conversation_id:'origin',attempt_id:'attempt-original',outcome:'completed' as const,control_cursor:'22',message_count:2,tool_call_count:0,
    final_message_id:null,final_message_locator:locator,final_message_id_deferred:false,final_message_id_source:null};
  const descriptor:RuntimeClientMessageDescriptor={locator,role:'assistant',kind:'message',block_count:'1',tool_call_count:'0',tool_calls:[],turn_process:owner,completed_response:null,content:[],collections:[]};
  const page:RuntimeClientTranscriptPage={entries:[],deferred_entries:[{offset:'21',identity:'native-ledger:17',value:descriptor,content:[],deferred:true}]};
  const original=JSON.stringify(page);
  const ui=render(<AgentTranscript snapshot={{...snapshot(),message_descriptors:[],transcript:page}}/>);
  expect(ui.getByRole('button',{name:'Worked'}).getAttribute('data-turn-process')).toBe(JSON.stringify(['origin','attempt-original']));
  expect(ui.container.querySelector('[data-native-final-message="true"]')).not.toBeNull();
  expect(isFinalTurnDescriptor({...descriptor,locator:{type:'by_ledger_position',position:18}})).toBe(false);
  for(const outcome of ['running','failed','cancelled','incomplete_at_cut'] as const)expect(isFinalTurnDescriptor({...descriptor,turn_process:{...owner,outcome}})).toBe(false);
  ui.rerender(<AgentTranscript snapshot={{...snapshot(),message_descriptors:[],transcript:{...page,deferred_entries:[{...page.deferred_entries![0]!,value:{...descriptor,turn_process:{...owner,outcome:'incomplete_at_cut'}}}]}}}/>);
  expect(ui.getByRole('button',{name:'Incomplete'})).toBeTruthy();
  expect(ui.queryByRole('button',{name:'Worked'})).toBeNull();
  expect(ui.container.querySelector('[data-native-final-message]')).toBeNull();
  expect(JSON.stringify(page)).toBe(original);
});


it('T08/T14 a deferred descriptor completion loads its original final metadata through the product reader',async()=>{
  const server=new Server();await server.attached('A');
  try {
    const base=snapshot(),target=server.target('A'),cut=checkpoint(base).read_cut;
    const locator={type:'by_ledger_position' as const,position:17};
    const raw='{ "id": "'+ 'original-final\\u0061'.repeat(40)+'", "opaque": 9007199254740993 }';
    const reference={conversation_id:base.conversation_id,source:{type:'canonical_message_block' as const,component:'message_metadata' as const,block_index:0,locator},representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:Buffer.byteLength(raw)};
    const owner={attempt_id_source:null,conversation_id:base.conversation_id,attempt_id:'exact-native-owner',outcome:'completed' as const,control_cursor:'22',message_count:2,tool_call_count:0,
      final_message_id:null,final_message_locator:locator,final_message_id_deferred:true,final_message_id_source:reference};
    const descriptor:RuntimeClientMessageDescriptor={locator,role:'assistant',kind:'message',block_count:'1',tool_call_count:'0',tool_calls:[],turn_process:owner,completed_response:null,content:[],collections:[]};
    server.handlers.set('conversation/contentRead',request=>{
      if(request.method!=='conversation/contentRead')throw Error('exact content method');
      expect(request.params).toEqual({target,agent_id:null,content_ref:reference,offset:0,max_bytes:65_536});
      return {type:'content_chunk',chunk:{content_ref:reference,offset:0,end_offset:reference.readable_end,readable_end:reference.readable_end,data_base64:Buffer.from(raw).toString('base64'),next_offset:null,complete:true}};
    });
    const state={...base,message_descriptors:[descriptor]},original=JSON.stringify(state);
    const ui=render(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>true}}><AgentTranscript snapshot={state}/></JournalSourceProvider>);
    await act(async()=>fireEvent.click(ui.getByRole('button',{name:`Read content (${reference.readable_end} bytes)`})));
    expect(ui.container.querySelector('pre')?.textContent).toBe(raw);
    expect(ui.container.querySelector('[data-native-final-message="true"]')).not.toBeNull();
    expect(state.attempt).toBeUndefined();expect(state.transcript.entries).toEqual([]);
    expect(JSON.stringify(state)).toBe(original);
  } finally {server.client.disconnect();}
});
