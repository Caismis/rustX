// @vitest-environment jsdom
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { OccupancyModelValue } from '../src/app/agent/OccupancyModelValue';
import { JournalSourceProvider } from '../src/app/agent/JournalValue';
import { checkpoint, Server, snapshot } from './fixture';
import type { RuntimeClientContextOccupancy } from '../../protocol/app-server/v48';
let server: Server;
afterEach(()=>{cleanup();server?.client.disconnect();});
const occupancy: RuntimeClientContextOccupancy = {estimated:false,input_tokens:'75',context_window_tokens:'100',
  model:null,model_deferred:true,model_source:'context.occupancy.inherited.model',
  breakdown:{system_tokens:'10',tool_tokens:'20',message_tokens:'45'}};

it('selected occupancy model reads exact original chunks only on explicit request', async()=>{
  server=new Server();await server.attached('A');
  const cut=checkpoint(snapshot()).read_cut,target=server.client.target('A');
  const original='"\uFEFFmodel\\u0061"',bytes=new TextEncoder().encode(original);
  server.handlers.set('conversation/checkpointValueRead',request=>{
    if(request.method!=='conversation/checkpointValueRead')throw new Error('method');
    const offset=request.params.offset,end=offset===0?3:bytes.length;
    expect(request.params).toEqual({target,agent_id:null,cut,domain:occupancy.model_source,item_offset:0,path:[],offset,max_bytes:65_536});
    return {type:'checkpoint_value_chunk',chunk:{cut,domain:occupancy.model_source!,item_offset:0,path:[],representation:'canonical_json_v1',
      offset,bytes_base64:Buffer.from(bytes.subarray(offset,end)).toString('base64'),readable_end:bytes.length,next_offset:end<bytes.length?end:null}};
  });
  const ui=render(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>true}}><OccupancyModelValue occupancy={occupancy}/></JournalSourceProvider>);
  expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read original value'})));
  expect(ui.container.querySelector('pre')?.textContent).toBe('"');
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next chunk'})));
  expect(ui.container.querySelector('pre')?.textContent).toBe(original.slice(1));
  expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(2);
});

it('replaced occupancy source never displays a late selected model response',async()=>{
  server=new Server();await server.attached('A');
  const cut=checkpoint(snapshot()).read_cut,target=server.client.target('A');
  server.held.add('conversation/checkpointValueRead');
  server.handlers.set('conversation/checkpointValueRead',request=>{
    if(request.method!=='conversation/checkpointValueRead')throw new Error('method');
    return {type:'checkpoint_value_chunk',chunk:{cut,domain:occupancy.model_source!,item_offset:0,path:[],representation:'canonical_json_v1',
      offset:0,bytes_base64:Buffer.from('"original"').toString('base64'),readable_end:10,next_offset:null}};
  });
  const source={client:server.client,target,cut:()=>cut,owned:()=>true};
  const ui=render(<JournalSourceProvider source={source}><OccupancyModelValue occupancy={occupancy}/></JournalSourceProvider>);
  fireEvent.click(ui.getByRole('button',{name:'Read original value'}));
  const request = await server.waitFor('conversation/checkpointValueRead', 1);
  ui.rerender(<JournalSourceProvider source={source}><OccupancyModelValue occupancy={{...occupancy,model_source:'context.occupancy.2.model'}}/></JournalSourceProvider>);
  await act(async()=>server.reply(request));
  expect(ui.container.querySelector('pre')).toBeNull();
});

it('cold occupancy reads the selected Node original model at its captured source cut without attaching',async()=>{
  server=new Server();await server.connect();
  const cut=checkpoint({...snapshot(),conversation_id:'historical-conversation'}).read_cut;
  const original='"original\\u0061-model"';
  server.handlers.set('session/traceValueRead',request=>{
    if(request.method!=='session/traceValueRead')throw new Error('method');
    expect(request.params).toEqual({session_id:'A',node_id:'historical-node',cut,domain:occupancy.model_source,item_offset:0,path:[],offset:0,max_bytes:65_536});
    return {type:'checkpoint_value_chunk',chunk:{cut,domain:occupancy.model_source!,item_offset:0,path:[],representation:'canonical_json_v1',offset:0,
      bytes_base64:Buffer.from(original).toString('base64'),readable_end:Buffer.byteLength(original),next_offset:null}};
  });
  const ui=render(<JournalSourceProvider source={{client:server.client,sessionId:'A',nodeId:'historical-node',cut:()=>cut,owned:()=>true}}><OccupancyModelValue occupancy={occupancy}/></JournalSourceProvider>);
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read original value'})));
  expect(ui.container.querySelector('pre')?.textContent).toBe(original);
  expect(server.requests.some(row=>row.request.method==='session/attach'||row.request.method==='conversation/checkpointValueRead')).toBe(false);
});
