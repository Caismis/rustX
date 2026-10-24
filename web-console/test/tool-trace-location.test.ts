import { afterEach, expect, it } from 'vitest';
import { Server, checkpoint } from './fixture';
import { AgentTraceReader } from '../src/client/agent-trace';
import { traceTool } from './trace-fixture';
import type {CheckpointPage,TraceInspectionHead,TraceFieldSource} from '../../protocol/app-server/v57';
import {traceInspectionHead} from '../../protocol/app-server/trace-inspection';
import {readTraceFieldWindow} from '../../protocol/app-server/trace-field';
import { refreshTrace, replaceTrace, selectTrace } from '../src/client/trace';
const locator = { occurrence: { locator: { type: 'by_message_id' as const, message_id: 'historical-assistant' }, block_index: 2 }, call_id: 'reused-call', tool_id: 'tool-bash' };
const child = {agent_id:'child',child_conversation_id:'conversation-child',activation_id:'activation-child',state:'active' as const};
const childCut = {conversation_id:child.child_conversation_id,activation_id:child.activation_id,revision:'1',through:'1',domain_revisions:{},collection_cuts:{}};
let server: Server;
const rootCut = () => checkpoint(server.client.getSnapshot().views.A.snapshot!).read_cut;
afterEach(() => server?.client.disconnect());
async function setup() { server = new Server(); await server.attached('A'); return server.client.getSnapshot().views.A.attachmentObservation!; }
it('native root location selects an unloaded record, retains its interval through refresh, and reads exact detail', async () => {
  await setup();
  server.handlers.set('session/traceLocateTool', request => {
    expect(request.params).toMatchObject({ locator });
    return { type: 'trace_tool_location', location: { record_id: 'trace:7', page: { read_cut: rootCut(), records: [traceTool(7)], next_cursor: 'trace:7' } } };
  });
  const cut={...rootCut(),revision:'7',through:'7'},original='{"name":"original-tool","opaque":18446744073709551615}';
  const field:Extract<TraceFieldSource,{type:'value'}>={type:'value',field:'definition',domain:'trace.records.2.detail.request.tools',item_offset:3,path:[],encoded_bytes:String(new TextEncoder().encode(original).length)};
  const head:TraceInspectionHead={id:'trace:7',kind:'tool',record:traceTool(7),content:{type:'tool',definition:{type:'available',field}}};
  const page:CheckpointPage={domain:'trace.records.7.detail',cursor:cut.revision,read_cut:cut,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'trace:7',key:null,value:JSON.parse(JSON.stringify(head)),content:[],deferred:true,body_complete:false}]};
  server.handlers.set('session/traceDetail', request => {expect(request.params).toMatchObject({target:server.client.target('A'),record_id:'trace:7'});return {type:'domain_page',page};});
  server.handlers.set('conversation/checkpointValueRead',request=>{expect(request.params).toEqual({target:server.client.target('A'),agent_id:null,cut,domain:field.domain,item_offset:field.item_offset,path:[],offset:0,max_bytes:65536});return {type:'checkpoint_value_chunk',chunk:{cut,domain:field.domain,item_offset:field.item_offset,path:[],offset:0,readable_end:new TextEncoder().encode(original).length,next_offset:null,representation:'canonical_json_v1',bytes_base64:btoa(original)}};});
  expect(await server.client.locateToolTrace('A', locator)).toBe(true);
  const landed = server.client.getSnapshot().views.A.trace!;
  expect(landed.selection?.id).toBe('trace:7');
  expect(landed.located).toBe(true);
  await server.client.loadTraceDetail('A', 'trace:7');
  const detail=server.client.getSnapshot().views.A.trace!.details['trace:7'];
  expect(detail.detail).toBeUndefined();expect(detail.error).toBeUndefined();expect(detail.page).toEqual(page);expect(detail.reader).toBeTruthy();
  const inspection=traceInspectionHead(detail.page!,'trace:7');expect(inspection?.id).toBe('trace:7');expect(inspection?.content).toEqual({type:'tool',definition:{type:'available',field}});
  expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
  const range=await readTraceFieldWindow(field,cut,0,offset=>detail.reader!.value(cut,field,offset,()=>true),()=>true);
  expect(new TextDecoder().decode(range.bytes)).toBe(original);expect(range.next).toBeUndefined();
  expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(1);
  const refreshed = refreshTrace(landed, { records: [traceTool(999)], next_cursor: 'trace:999' });
  expect(refreshed.page.records.map(record => record.id)).toEqual(['trace:7']);
  expect(refreshed.selection?.id).toBe('trace:7');
  server.handlers.set('session/trace', () => ({ type: 'trace', page: { read_cut: rootCut(), records: [traceTool(999)] } }));
  await server.client.returnToLatestTrace('A');
  expect(server.client.getSnapshot().views.A.trace?.located).toBeUndefined();
});
it('later root navigation supersedes a delayed historical location', async () => {
  await setup(); server.held.add('session/traceLocateTool');
  const first = server.client.locateToolTrace('A', locator);
  const firstRequest = await server.waitFor('session/traceLocateTool', 1);
  const second = server.client.locateToolTrace('A', { ...locator, call_id: 'second' });
  const secondRequest = await server.waitFor('session/traceLocateTool', 2);
  server.handlers.set('session/traceLocateTool', () => ({ type: 'trace_tool_location', location: { record_id: 'trace:8', page: { read_cut: rootCut(), records: [traceTool(8)], next_cursor: null } } }));
  server.reply(secondRequest); expect(await second).toBe(true);
  server.handlers.set('session/traceLocateTool', () => ({ type: 'trace_tool_location', location: { record_id: 'trace:7', page: { read_cut: rootCut(), records: [traceTool(7)], next_cursor: null } } }));
  server.reply(firstRequest); expect(await first).toBe(false);
  expect(server.client.getSnapshot().views.A.trace?.selection?.id).toBe('trace:8');
});
it('child location uses only the registered child authority and survives live updates and latest recovery', async () => {
  const proof = await setup(); const reader = new AgentTraceReader(server.client, 'A', proof, child);
  server.handlers.set('agent/traceLocateTool', request => {
    expect(request.params).toMatchObject({ agent_id: 'child', locator, target: proof.target });
    return { type: 'trace_tool_location', location: { record_id: 'trace:7', page: { read_cut: childCut, records: [traceTool(7)], next_cursor: 'trace:7' } } };
  });
  server.handlers.set('agent/trace', () => ({ type: 'trace', page: { read_cut: childCut, records: [traceTool(999)], next_cursor: null } }));
  expect(await reader.locate(locator)).toBe(true); await reader.refresh();
  expect(reader.snapshot().selection?.id).toBe('trace:7');
  expect(reader.snapshot().page.records.map(record => record.id)).toEqual(['trace:7']);
  await reader.latest();
  expect(reader.snapshot().located).toBeUndefined();
  expect(reader.snapshot().page.records.map(record => record.id)).toEqual(['trace:999']);
  expect(server.requests.some(row => row.request.method === 'session/traceLocateTool')).toBe(false);
  reader.retire();
});
it('retired attachments reject late root and child locations', async () => {
  const proof = await setup(), reader = new AgentTraceReader(server.client, 'A', proof, child);
  for (const method of ['session/traceLocateTool', 'agent/traceLocateTool'] as const) {
    server.held.add(method); server.handlers.set(method, () => ({ type: 'trace_tool_location', location: { record_id: 'trace:7', page: { read_cut: method === 'agent/traceLocateTool' ? childCut : rootCut(), records: [traceTool(7)] } } }));
  }
  const work = Promise.allSettled([server.client.locateToolTrace('A', locator), reader.locate(locator)]);
  const r = await server.waitFor('session/traceLocateTool', 1), c = await server.waitFor('agent/traceLocateTool', 1), socket = server.socket;
  server.client.disconnect(); reader.retire(); server.reply(r, socket); server.reply(c, socket);
  expect((await work).every(result => result.status === 'rejected')).toBe(true);
  expect(server.client.getSnapshot().uncertain).toEqual([]);
  expect(reader.snapshot().selection).toBeUndefined();
});
it('no execution evidence does not select a neighbouring or proposed call', async () => {
  await setup(); server.handlers.set('session/traceLocateTool', () => ({ type: 'trace_tool_location', location: null }));
  await expect(server.client.locateToolTrace('A', locator)).rejects.toThrow('no native execution record');
  expect(server.client.getSnapshot().views.A.trace?.selection).toBeUndefined();
});
it('located records still receive native lifecycle repairs without concatenating a discontinuous latest page', () => {
  const record = traceTool(7, { state: 'running' });
  const cache = selectTrace({ ...replaceTrace({ records: [record] }), located: true as const }, record.id);
  const next = refreshTrace(cache, { records: [traceTool(999)] }, [{ ...record, state: 'completed' }]);
  expect(next.selection?.state).toBe('completed');
  expect(next.page.records.map(row => row.id)).toEqual(['trace:7']);
});
