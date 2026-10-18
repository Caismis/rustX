import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { AttemptIdentityValue, AttemptJournalValues, JournalSourceProvider, JournalValue, ToolMetadataValue, TurnFinalMessageValue } from '../src/app/agent/JournalValue';
import { Tool } from '../src/app/agent/Tool';
import { checkpoint, Server, snapshot } from './fixture';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });

it('T08 a deferred Tool result displays exact original opaque JSON bytes without rounding or reserialization', async () => {
  server = new Server(); await server.attached('A');
  const raw = '{ "wide": 9007199254740993, "escape": "\\u0061" }';
  const cut = checkpoint(snapshot()).read_cut, target = server.client.target('A');
  const field = { sequence: 1, path: ['event', 'result'], encoded_bytes: new TextEncoder().encode(raw).length };
  server.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw new Error('method');
    expect(request.params).toEqual({ target, agent_id: null, cut, domain: 'journal.1', item_offset: 0, path: field.path, offset: 0, max_bytes: 65_536 });
    return { type: 'checkpoint_value_chunk', chunk: { cut, domain: 'journal.1', item_offset: 0, path: field.path, representation: 'canonical_json_v1', offset: 0, bytes_base64: btoa(raw), readable_end: field.encoded_bytes, next_offset: null } };
  });
  const ui = render(<JournalSourceProvider source={{ client: server.client, target, cut: () => cut, owned: () => true }}><JournalValue field={field}/></JournalSourceProvider>);
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Read original value' })));
  expect(ui.container.querySelector('pre')?.textContent).toBe(raw);
});

it('T11 hiding the Tool result owner fences a physically late source response', async () => {
  server = new Server(); await server.attached('A'); server.held.add('conversation/checkpointValueRead');
  const cut = checkpoint(snapshot()).read_cut, target = server.client.target('A');
  let visible = true;
  const field = { sequence: 1, path: ['event', 'result'], encoded_bytes: 8 };
  const ui = render(<JournalSourceProvider source={{ client: server.client, target, cut: () => cut, owned: () => visible }}><JournalValue field={field}/></JournalSourceProvider>);
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Read original value' })));
  const request = await server.waitFor('conversation/checkpointValueRead', 1);
  visible = false;
  await act(async () => server.socket.success(request, { type: 'checkpoint_value_chunk', chunk: { cut, domain: 'journal.1', item_offset: 0, path: field.path, representation: 'canonical_json_v1', offset: 0, bytes_base64: btoa('obsolete'), readable_end: 8, next_offset: null } }));
  expect(ui.queryByText('obsolete')).toBeNull();
});

it('T08 canonical-only Tool results load original metadata bytes through the bounded content source', async () => {
  server = new Server(); await server.attached('A');
  const target = server.client.target('A'), cut = checkpoint(snapshot()).read_cut;
  const raw = '{"result":{ "wide": 9007199254740993, "escape": "\\u0061" }}';
  const reference = {conversation_id:target.conversation_id,source:{type:'canonical_message_block' as const,component:'block_metadata' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:new TextEncoder().encode(raw).length};
  server.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw new Error('method');
    expect(request.params.content_ref).toEqual(reference); expect(request.params.offset).toBe(0); expect(request.params.max_bytes).toBe(65_536);
    return {type:'content_chunk',chunk:{content_ref:reference,offset:0,end_offset:reference.readable_end,readable_end:reference.readable_end,data_base64:btoa(raw),next_offset:null,complete:true}};
  });
  const ui=render(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>true}}><ToolMetadataValue reference={reference}/></JournalSourceProvider>);
  await act(async()=>fireEvent.click(ui.getAllByRole('button')[0]!));
  expect(ui.container.querySelector('pre')?.textContent).toBe(raw);
});

it('T11 a complete inline C result keeps its exact source without requesting duplicate bytes', async () => {
  server = new Server(); await server.attached('A');
  const target = server.client.target('A'), cut = checkpoint(snapshot()).read_cut;
  const reference = {conversation_id:target.conversation_id,source:{type:'canonical_message_block' as const,component:'block_metadata' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:100};
  const tool = {occurrence:{locator:{type:'by_ledger_position' as const,position:1},block_index:0},block_index:0,
    name:'bash',assembly_complete:true,committed_result_deferred:false,result_metadata_ref:reference,
    state:{type:'running' as const,arguments:'{"command":"test"}'},
    committed_result:{status:{type:'success' as const},content:[{type:'text' as const,text:'inline canonical body'}],duration_ms:'1'}};
  const source = {client:server.client,target,cut:()=>cut,owned:()=>true};
  const ui = render(<JournalSourceProvider source={source}><Tool tool={tool}/></JournalSourceProvider>);
  fireEvent.click(ui.container.querySelector('[data-disclosure-row]')!);
  expect(ui.getByText('inline canonical body')).toBeTruthy();
  expect(ui.queryByRole('button',{name:/100/})).toBeNull();
  expect(ui.container.querySelector('[data-tool-result-committed]')).not.toBeNull();
  ui.rerender(<JournalSourceProvider source={source}><Tool tool={{...tool,committed_result_deferred:true}}/></JournalSourceProvider>);
  expect(ui.getByRole('button',{name:/100/})).toBeTruthy();
  expect(server.requests.filter(row=>row.request.method==='conversation/contentRead')).toHaveLength(0);
  expect(tool.state.type).toBe('running');
});

it.each(['failed', 'completed'] as const)('T08/T19 %s diagnostics read their native Journal source without reconstructing an outcome', async type => {
  server = new Server(); await server.attached('A');
  const target = server.target('A'), cut = checkpoint(snapshot()).read_cut;
  const raw = '"original \\u0061 diagnostic"', field = {sequence: 5, path: ['normalized_failure'], encoded_bytes: new TextEncoder().encode(raw).length};
  const outcome = type === 'failed'
    ? {type, error: {type: 'model' as const, kind: 'provider_error' as const, message: 'preview', retry_after_ms: null}, error_deferred: true, error_source: field}
    : {type, finish_reason: {type: 'other' as const, reason: 'preview'}, finish_reason_deferred: true, finish_reason_source: field};
  const attempt = {attempt_id: 'a', turn: 1, phase: {type: 'settled' as const, outcome}};
  server.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw Error('Wrong method');
    expect(request.params.domain).toBe('journal.5');
    return {type: 'checkpoint_value_chunk', chunk: {cut, domain: 'journal.5', item_offset: 0, path: field.path, representation: 'canonical_json_v1', offset: 0, bytes_base64: btoa(raw), readable_end: field.encoded_bytes, next_offset: null}};
  });
  const ui = render(<JournalSourceProvider source={{client: server.client, target, cut: () => cut, owned: () => true}}><AttemptJournalValues attempt={attempt}/></JournalSourceProvider>);
  await act(async () => fireEvent.click(ui.getByRole('button', {name: 'Read original value'})));
  expect(ui.container.querySelector('pre')?.textContent).toBe(raw);
  expect(attempt.phase.outcome).toBe(outcome);
});

it('T12 a substituted Journal representation is rejected before its bytes enter the view', async () => {
  server = new Server(); await server.attached('A');
  const target = server.target('A'), cut = checkpoint(snapshot()).read_cut;
  const field = {sequence: 5, path: ['normalized_failure'], encoded_bytes: 8};
  server.handlers.set('conversation/checkpointValueRead', () => ({type: 'checkpoint_value_chunk', chunk: {cut, domain: 'journal.5', item_offset: 0, path: field.path, representation: 'utf8_text', offset: 0, bytes_base64: btoa('injected'), readable_end: 8, next_offset: null}}));
  const ui = render(<JournalSourceProvider source={{client: server.client, target, cut: () => cut, owned: () => true}}><JournalValue field={field}/></JournalSourceProvider>);
  await act(async () => fireEvent.click(ui.getByRole('button', {name: 'Read original value'})));
  expect(ui.queryByText('injected')).toBeNull();
  expect(ui.getByRole('alert').textContent).toContain('Native journal field range changed');
});

it('a deferred cold turn identity retains its original StartJ cut across continuation after a model change', async () => {
  server=new Server();await server.connect();
  const originalCut=checkpoint({...snapshot(),conversation_id:'historical-conversation'}).read_cut;
  let currentCut=originalCut;
  const raw='"'+ 'original\\u0061-attempt'.repeat(40)+'"',field={sequence:7,path:['attempt_id'],encoded_bytes:Buffer.byteLength(raw)};
  server.handlers.set('session/traceValueRead',request=>{
    if(request.method!=='session/traceValueRead')throw Error('method');
    const offset=request.params.offset,end=offset===0?5:field.encoded_bytes;
    expect(request.params).toEqual({session_id:'A',node_id:'historical-node',cut:originalCut,domain:'journal.7',item_offset:0,path:field.path,offset,max_bytes:65_536});
    return {type:'checkpoint_value_chunk',chunk:{cut:originalCut,domain:'journal.7',item_offset:0,path:field.path,representation:'canonical_json_v1',offset,
      bytes_base64:Buffer.from(raw.slice(offset,end)).toString('base64'),readable_end:field.encoded_bytes,next_offset:end<field.encoded_bytes?end:null}};
  });
  const ui=render(<JournalSourceProvider source={{client:server.client,sessionId:'A',nodeId:'historical-node',cut:()=>currentCut,owned:()=>true}}><JournalValue field={field}/></JournalSourceProvider>);
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read original value'})));
  expect(ui.container.querySelector('pre')?.textContent).toBe(raw.slice(0,5));
  currentCut={...originalCut,revision:'2',through:'2'};
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next chunk'})));
  expect(ui.container.querySelector('pre')?.textContent).toBe(raw.slice(5));
  expect(server.requests.some(row=>row.request.method==='session/attach')).toBe(false);
});

it('final-message metadata reads original bytes at its native source and a retired view rejects late bytes',async()=>{
 server=new Server();await server.attached('A');
 const target=server.target('A'),cut=checkpoint(snapshot()).read_cut,raw=JSON.stringify({id:'original-final'.repeat(40),timestamp:'2026-10-14T00:00:00Z'});
 const reference={conversation_id:target.conversation_id,source:{type:'canonical_message_block' as const,component:'message_metadata' as const,block_index:0,locator:{type:'by_ledger_position' as const,position:17}},representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:Buffer.byteLength(raw)};
 let expected=reference;
 server.handlers.set('conversation/contentRead',request=>{
   if(request.method!=='conversation/contentRead')throw Error('method');
   expect(request.params).toEqual({target,agent_id:null,content_ref:expected,offset:0,max_bytes:65_536});
   return {type:'content_chunk',chunk:{content_ref:expected,offset:0,end_offset:expected.readable_end,readable_end:expected.readable_end,data_base64:Buffer.from(raw).toString('base64'),next_offset:null,complete:true}};
 });
 let owned=true;
 const ui=render(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>owned}}><TurnFinalMessageValue reference={reference}/></JournalSourceProvider>);
 await act(async()=>fireEvent.click(ui.getAllByRole('button')[0]!));
 expect(ui.container.querySelector('pre')?.textContent).toBe(raw);
 server.held.add('conversation/contentRead');
 expected={...reference,source:{...reference.source,locator:{type:'by_ledger_position',position:18}}};
 ui.rerender(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>owned}}><TurnFinalMessageValue reference={expected}/></JournalSourceProvider>);
 fireEvent.click(ui.getAllByRole('button')[0]!);
 const request=await server.waitFor('conversation/contentRead',2);owned=false;
 await act(async()=>server.reply(request));
 expect(ui.container.querySelector('pre')).toBeNull();
});

it('cold final metadata uses its advertised historical source cut without substituting a live attachment',async()=>{
 server=new Server();await server.connect();
 const sourceCut={read:{conversation_id:'historical',journal:'7',transcript:'9',mutation_revision:'0',ledger_through:'17',surface_revision:'0',transcript_total:'9',pending_through:'0',pending_total:'0'},ledger_through:'17',transcript_total:'9',surface_revision:'0',inherited_through:'0'};
 const raw=JSON.stringify({id:'original-final'.repeat(40)}),reference={conversation_id:'historical',source:{type:'canonical_message_block' as const,component:'message_metadata' as const,block_index:0,locator:{type:'by_ledger_position' as const,position:17}},representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:Buffer.byteLength(raw)};
 server.handlers.set('session/historyContentRead',request=>{
   if(request.method!=='session/historyContentRead')throw Error('method');
   expect(request.params).toEqual({session_id:'A',node_id:'historical-node',cut:sourceCut,content_ref:reference,offset:0,max_bytes:65_536});
   return {type:'content_chunk',chunk:{content_ref:reference,offset:0,end_offset:reference.readable_end,readable_end:reference.readable_end,data_base64:Buffer.from(raw).toString('base64'),next_offset:null,complete:true}};
 });
 const ui=render(<JournalSourceProvider source={{client:server.client,sessionId:'A',nodeId:'historical-node',historyCut:sourceCut,cut:()=>undefined,owned:()=>true}}><TurnFinalMessageValue reference={reference}/></JournalSourceProvider>);
 await act(async()=>fireEvent.click(ui.getAllByRole('button')[0]!));
 expect(ui.container.querySelector('pre')?.textContent).toBe(raw);
 expect(server.requests.some(row=>row.request.method==='session/attach'||row.request.method==='conversation/contentRead')).toBe(false);
});

it('T08/T14 inherited original owner bytes use the advertised domain and frozen selected cut, never a fake Journal',async()=>{
 server=new Server();await server.connect();
 const originalCut=checkpoint({...snapshot(),conversation_id:'current-child-source'}).read_cut;let currentCut=originalCut;
 const raw='"'+ 'original\\u0061-owner'.repeat(40)+'"';
 const field={type:'inherited' as const,kind:'turn' as const,ordinal:0,domain:'transcript.turns.0.identity',encoded_bytes:Buffer.byteLength(raw)};
 server.handlers.set('session/traceValueRead',request=>{
   if(request.method!=='session/traceValueRead')throw Error('exact cold selected read');
   const offset=request.params.offset,end=offset===0?5:field.encoded_bytes;
   expect(request.params).toEqual({session_id:'A',node_id:'current-child-node',cut:originalCut,domain:field.domain,item_offset:0,path:['attempt_id'],offset,max_bytes:65_536});
   return {type:'checkpoint_value_chunk',chunk:{cut:originalCut,domain:field.domain,item_offset:0,path:['attempt_id'],representation:'canonical_json_v1',offset,
     bytes_base64:Buffer.from(raw.slice(offset,end)).toString('base64'),readable_end:field.encoded_bytes,next_offset:end<field.encoded_bytes?end:null}};
 });
 const ui=render(<JournalSourceProvider source={{client:server.client,sessionId:'A',nodeId:'current-child-node',cut:()=>currentCut,owned:()=>true}}><AttemptIdentityValue field={field}/></JournalSourceProvider>);
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read original value'})));
 expect(ui.container.querySelector('pre')?.textContent).toBe(raw.slice(0,5));
 currentCut={...originalCut,revision:'2',through:'2'};
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next chunk'})));
 expect(ui.container.querySelector('pre')?.textContent).toBe(raw.slice(5));
 expect(server.requests.some(row=>row.request.method==='session/attach'||'domain' in row.request.params&&String(row.request.params.domain).startsWith('journal.'))).toBe(false);
});
