import {act,cleanup,fireEvent,render,screen,waitFor,within} from '@testing-library/react';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,CheckpointReadCut,MethodResult,Request} from '../../protocol/app-server/v57';
import {Server,checkpoint,snapshot} from './fixture';
import {nativeTraceFixture} from './native-trace-fixture';
import {traceRecord,requestDetail} from './trace-fixture';
import {AgentTraceReader} from '../src/client/agent-trace';
import {Trajectory} from '../src/app/trajectory/Trajectory';
import type {TraceDetailEntry} from '../src/client/trace';
const servers:Server[]=[];
beforeEach(()=>{vi.spyOn(HTMLElement.prototype,'offsetHeight','get').mockReturnValue(360);vi.spyOn(HTMLElement.prototype,'offsetWidth','get').mockReturnValue(800);});
afterEach(()=>{cleanup();servers.splice(0).forEach(server=>server.client.disconnect());vi.restoreAllMocks();});
const record=()=>{const row=traceRecord(4);row.request!.model='console-model';return row;};
const descriptor=(cut:CheckpointReadCut):CheckpointPage=>({cursor:cut.revision,domain:'trace.records.4.detail',read_cut:cut,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'trace:4',key:null,value:{id:'trace:4',kind:'request',request:{contributions:[{producer:{Native:'agent_status'}}]},truncated:true},deferred:true,content:[]}]});
function fieldPage(request:Request,total='0'):MethodResult {
 if(request.method!=='conversation/checkpointPage'&&request.method!=='session/tracePage')throw Error('page method');
 return {type:'checkpoint_page',page:{cursor:request.params.cut.revision,domain:request.params.domain,read_cut:request.params.cut,total,next_offset:null,collections:[],items:total==='0'?[]:[{offset:'0',identity:'options.0',key:null,value:null,deferred:true,content:[]}]}};
}
function valueReply(request:Request,text='"console-model"'):MethodResult {
 if(request.method!=='conversation/checkpointValueRead'&&request.method!=='session/traceValueRead')throw Error('value method');
 const p=request.params,bytes=new TextEncoder().encode(text),end=Math.min(bytes.length,p.offset+p.max_bytes),part=bytes.slice(p.offset,end);
 return {type:'checkpoint_value_chunk',chunk:{cut:p.cut,domain:p.domain,item_offset:p.item_offset,path:p.path,offset:p.offset,readable_end:bytes.length,next_offset:end<bytes.length?end:null,representation:'canonical_json_v1',bytes_base64:btoa(String.fromCharCode(...part))}};
}
async function root(){
 const server=new Server();servers.push(server);server.snapshots.set('A',{...snapshot(),trace:{records:[record()],next_cursor:null}});
 const cut={...checkpoint(snapshot()).read_cut,revision:'9',through:'7'};
 server.handlers.set('session/traceDetail',()=>({type:'domain_page',page:descriptor(cut)}));
 const original=requestDetail(4);original.request!.model='console-model';const native=nativeTraceFixture(record(),original,cut);
 server.handlers.set('conversation/checkpointPage',request=>{if(request.method!=='conversation/checkpointPage')throw Error('page');return {type:'checkpoint_page',page:native.original.page(request.params.cut,request.params.domain,request.params.offset,()=>true)};});
 server.handlers.set('conversation/checkpointValueRead',request=>{if(request.method!=='conversation/checkpointValueRead')throw Error('value');return {type:'checkpoint_value_chunk',chunk:native.original.value(request.params.cut,request.params,request.params.offset,()=>true)};});
 await server.attached('A');await server.client.loadTraceDetail('A','trace:4');server.client.selectTrace('A','trace:4');
 return {server,cut};
}
function show(server:Server){const load=vi.fn();render(<Trajectory cache={server.client.getSnapshot().views.A.trace!} loadEarlier={()=>{}} onSelect={()=>{}} onLoadDetail={load}/>);fireEvent.click(screen.getByRole('tab',{name:'Options'}));return load;}
it('retains a positive deferred header and reads selected Options at its own returned cut',async()=>{
 const {server,cut}=await root(),entry=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 expect(entry.page).toEqual(descriptor(cut));expect(entry.detail).toBeUndefined();expect(entry.error).toBeUndefined();
 const load=show(server);expect(screen.queryByRole('alert')).toBeNull();expect(load).not.toHaveBeenCalled();
 await screen.findByText('"console-model"');expect(screen.queryByRole('combobox')).toBeNull();
 await waitFor(()=>expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(6+Math.max(1,requestDetail(4).request!.options.length)));
 const reads=server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead');
 expect(reads.every(row=>'cut' in row.request.params&&JSON.stringify(row.request.params.cut)===JSON.stringify(cut))).toBe(true);
 expect(reads[0].request.params).toMatchObject({cut,domain:'trace.records.4.detail.request.model',item_offset:0,path:[],offset:0,max_bytes:65536});
 await server.client.loadTraceDetail('A','trace:4');expect(server.requests.filter(row=>row.request.method==='session/traceDetail')).toHaveLength(1);
});
it('automatically assembles original Options with exact numbers and no field selector',async()=>{
 const {server}=await root(),prefix='{"text":"',value=prefix+'x'.repeat(65535-prefix.length)+'🙂","opaque":18446744073709551615}';
 const previous=server.handlers.get('conversation/checkpointValueRead')!;
 server.handlers.set('conversation/checkpointValueRead',request=>request.method==='conversation/checkpointValueRead'&&request.params.domain.endsWith('.options')?valueReply(request,JSON.stringify({name:'native-option',value:{value:null,truncated:false}}).replace('null',value)):previous(request));
 show(server);await screen.findByText('native-option:');await screen.findByText('18446744073709551615');
 expect(document.body.textContent).not.toContain('18446744073709552000');expect(screen.queryByRole('combobox')).toBeNull();expect(screen.queryByRole('button',{name:/Read original|Read next chunk/})).toBeNull();
});
it.each(['root','child','historical'] as const)('%s keeps its own page cut and suppresses replies after its owner is replaced',async kind=>{
 let server:Server,entry:TraceDetailEntry,retire:()=>Promise<void>,cut:CheckpointReadCut;
 if(kind==='root'){
  ({server,cut}=await root());entry=server.client.getSnapshot().views.A.trace!.details['trace:4'];retire=()=>server.client.release('A');
 }else if(kind==='child'){
  server=new Server();servers.push(server);await server.attached('A');
  cut={...checkpoint(snapshot('child')).read_cut,activation_id:'activation-child',revision:'12',through:'8'};
  server.handlers.set('agent/trace',()=>({type:'trace',page:{read_cut:cut,records:[record()],next_cursor:null}}));
  server.handlers.set('agent/traceDetail',()=>({type:'domain_page',page:descriptor(cut)}));
  const child=new AgentTraceReader(server.client,'A',server.client.getSnapshot().views.A.attachmentObservation!,{agent_id:'agent-child',activation_id:'activation-child',child_conversation_id:cut.conversation_id,state:'active'});
  await child.refresh();await child.detail('trace:4');entry=child.snapshot().details['trace:4'];retire=async()=>child.retire();
 }else{
  server=new Server();servers.push(server);await server.connect();server.held.add('session/attach');
  cut={...checkpoint(snapshot()).read_cut,revision:'0',through:'7'};
  server.handlers.set('session/traceHistory',()=>({type:'session_trace_history',conversation_id:cut.conversation_id,page:{read_cut:cut,records:[record()],next_cursor:null}}));
  server.handlers.set('session/traceHistoryDetail',()=>({type:'session_trace_history_detail',conversation_id:cut.conversation_id,page:descriptor(cut)}));
  const opening=server.client.attach('A'),request=await server.waitFor('session/attach',1);
  await waitFor(()=>expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records).toHaveLength(1));
  await server.client.loadTraceDetail('A','trace:4');entry=server.client.getSnapshot().views.A.tracePreview!.cache.details['trace:4'];retire=async()=>{server.reply(request);await opening;};
 }
 expect(entry.error).toBeUndefined();expect(entry.page).toEqual(descriptor(cut));expect(entry.reader).toBeTruthy();
 const method=kind==='historical'?'session/traceValueRead':'conversation/checkpointValueRead';server.held.add(method);
 const field={domain:'trace.records.4.detail.request.model',item_offset:0,path:[]};
 const pending=entry.reader!.value(cut,field,0,()=>true),rejected=expect(pending).rejects.toThrow();const request=await server.waitFor(method,1);
 expect(request.params).toMatchObject({cut,...field,offset:0,max_bytes:65536});if(kind==='child')expect(request.params).toMatchObject({agent_id:'agent-child'});
 await act(retire);server.socket.success(request,valueReply(request));await rejected;
 await expect(entry.reader!.value(cut,field,0,()=>true)).rejects.toThrow('replaced');expect(server.requests.filter(row=>row.request.method===method)).toHaveLength(1);
});
it('Diff selects the exact native predecessor prompt at the current retained detail cut',async()=>{
 const {server,cut}=await root(),earlier=traceRecord(2),current=record();earlier.request!.request_id='original-prior';current.request!.predecessor={availability:'available',request_id:'original-prior',request_id_source:null};current.request!.system_prompt.state='changed';
 const cache=server.client.getSnapshot().views.A.trace!;cache.page.records=[earlier,current];cache.selection=current;
 const oldDetail=requestDetail(2),newDetail=requestDetail(4);oldDetail.request!.effective_system_prompt.text='ORIGINAL_PRIOR_PROMPT';newDetail.request!.effective_system_prompt.text='CURRENT_PROMPT';
 const oldSource=nativeTraceFixture(earlier,oldDetail,cut),newSource=nativeTraceFixture(current,newDetail,cut);
 server.handlers.set('conversation/checkpointPage',request=>{if(request.method!=='conversation/checkpointPage')throw Error('page');const source=request.params.domain.startsWith('trace.records.2.')?oldSource:newSource;return {type:'checkpoint_page',page:source.original.page(cut,request.params.domain,request.params.offset,()=>true)};});
 server.handlers.set('conversation/checkpointValueRead',request=>{if(request.method!=='conversation/checkpointValueRead')throw Error('value');const source=request.params.domain.startsWith('trace.records.2.')?oldSource:newSource;return {type:'checkpoint_value_chunk',chunk:source.original.value(cut,request.params,request.params.offset,()=>true)};});
 render(<Trajectory cache={cache} loadEarlier={()=>{}} onSelect={()=>{}} onLoadDetail={()=>{}}/>);
 fireEvent.click(document.querySelector('[data-display-type="SystemPromptCell"][data-owner="trace:4"]')!);fireEvent.click(screen.getByRole('tab',{name:'Diff'}));
 await screen.findByText('-ORIGINAL_PRIOR_PROMPT');await screen.findByText('+CURRENT_PROMPT');
 const read=server.requests.find(row=>row.request.method==='conversation/checkpointValueRead'&&row.request.params.domain.startsWith('trace.records.2.'))!.request;
 expect(read.params).toMatchObject({cut,domain:'trace.records.2.detail.request.effective_system_prompt',item_offset:0,path:[],offset:0,max_bytes:65536});
 expect(server.requests.filter(row=>row.request.method==='session/traceDetail')).toHaveLength(1);
});
it('a missing loaded predecessor offers the existing earlier-history gesture without claiming source absence',async()=>{
 const {server}=await root(),cache=server.client.getSnapshot().views.A.trace!,row=cache.page.records[0];row.request!.system_prompt.state='changed';cache.selection=row;cache.page.next_cursor='trace:4';const earlier=vi.fn();
 render(<Trajectory cache={cache} loadEarlier={earlier} onSelect={()=>{}} onLoadDetail={()=>{}}/>);
 fireEvent.click(document.querySelector('[data-display-type="SystemPromptCell"][data-owner="trace:4"]')!);fireEvent.click(screen.getByRole('tab',{name:'Diff'}));
 expect(screen.getByText('Previous request is not loaded; load earlier Trace history.')).toBeTruthy();
 fireEvent.click(within(screen.getByRole('status')).getByRole('button',{name:'Load earlier history'}));expect(earlier).toHaveBeenCalledTimes(1);
 expect(screen.queryByRole('alert')).toBeNull();expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
});
it('detail descriptor replacement invalidates the reader while the existing finite cache retains headers within its count bound',async()=>{
 const {server,cut}=await root(),entry=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 const {completeTraceDetail,replaceTrace,TRACE_DETAIL_LIMIT}=await import('../src/client/trace');
 let cache=replaceTrace({records:Array.from({length:12},(_,n)=>traceRecord(n+1)),next_cursor:null});
 for(let n=1;n<=12;n++){const page={...descriptor(cut),domain:`trace.records.${n}.detail`,items:[{...descriptor(cut).items[0],identity:`trace:${n}`,value:{id:`trace:${n}`,request:{model:'original model'}}}]};cache=completeTraceDetail(cache,`trace:${n}`,cache.epoch,undefined,undefined,undefined,undefined,page);}
 expect(Object.keys(cache.details)).toHaveLength(TRACE_DETAIL_LIMIT);expect(Object.values(cache.details).every(row=>row.page&&!row.detail&&!row.error)).toBe(true);
 const detail=server.client.getSnapshot().views.A.trace!.details;
 // Same epoch/owner, a different retained detail page still cannot authorize the old reader.
 server.client.getSnapshot().views.A.trace!.details={...detail,'trace:4':{...entry,page:structuredClone(entry.page)}};
 await expect(entry.reader!.value(cut,{domain:'trace.records.4.detail.request.model',item_offset:0,path:[]},0,()=>true)).rejects.toThrow('replaced');
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
});
it('late selected field pages cannot publish into another explicit field selection',async()=>{
 const {server}=await root();server.held.add('conversation/checkpointPage');show(server);
 const request=await server.waitFor('conversation/checkpointPage',1);
 fireEvent.click(screen.getByRole('tab',{name:'Timing'}));
 await act(async()=>server.socket.success(request,fieldPage(request)));
 expect(screen.queryByRole('alert')).toBeNull();expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
 expect(screen.getByRole('tab',{name:'Timing'}).getAttribute('aria-selected')).toBe('true');
});
it('an original field continuation rejects changed or unsafe authoritative byte extents',async()=>{
 const {readTraceSelectedWindow}=await import('../../protocol/app-server/trace-field');
 const cut=checkpoint(snapshot()).read_cut,selector={domain:'trace.records.4.detail.request.model',item_offset:0,path:[]};
 const chunk={cut,...selector,offset:0,readable_end:2,next_offset:1,representation:'canonical_json_v1' as const,bytes_base64:btoa('"')};
 const first=await readTraceSelectedWindow(selector,cut,0,async()=>chunk,()=>true);expect(first.readable_end).toBe(2);
 await expect(readTraceSelectedWindow(selector,cut,1,async()=>({...chunk,offset:1,readable_end:3,next_offset:2}),()=>true,first.readable_end)).rejects.toThrow('extent changed');
 await expect(readTraceSelectedWindow(selector,cut,0,async()=>({...chunk,readable_end:Number.MAX_SAFE_INTEGER+1}),()=>true)).rejects.toThrow('extent changed');
});
it('a foreign deferred descriptor is an explicit unavailable error rather than a retained positive header',async()=>{
 const {server,cut}=await root();await server.client.release('A');await server.client.attach('A');
 server.handlers.set('session/traceDetail',()=>({type:'domain_page',page:descriptor({...cut,conversation_id:'foreign'})}));
 await server.client.loadTraceDetail('A','trace:4');const entry=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 expect(entry.page).toBeUndefined();expect(entry.detail).toBeUndefined();expect(entry.error).toContain('source changed');
});
it('checkpoint authority replacement evicts a deferred reader and permits a fresh original detail selection',async()=>{
 const {server,cut}=await root(),first=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 server.held.add('conversation/checkpointValueRead');const field={domain:'trace.records.4.detail.request.model',item_offset:0,path:[]};
 const pending=first.reader!.value(cut,field,0,()=>true),rejected=expect(pending).rejects.toThrow();const request=await server.waitFor('conversation/checkpointValueRead',1);
 await server.client.refresh('A');expect(server.client.getSnapshot().views.A.trace!.details['trace:4']).toBeUndefined();
 server.socket.success(request,valueReply(request));await rejected;
 await server.client.loadTraceDetail('A','trace:4');const next=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 expect(next.page).toEqual(descriptor(cut));expect(next.page).not.toBe(first.page);expect(next.error).toBeUndefined();
 server.held.delete('conversation/checkpointValueRead');server.handlers.set('conversation/checkpointValueRead',request=>valueReply(request));
 expect(await next.reader!.value(cut,field,0,()=>true)).toMatchObject({cut,domain:field.domain});
 expect(server.requests.filter(row=>row.request.method==='session/traceDetail')).toHaveLength(2);
});
it.each([false,true])('checkpoint refresh retains inline bytes only when no fenced content reader is required (bindings=%s)',async bindings=>{
 const {server,cut}=await root();await server.client.refresh('A');
 const ref={conversation_id:'conversation-A',source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'utf8_text' as const,version:1,content_revision:1,readable_end:4};
 const page=descriptor(cut);page.items[0]={...page.items[0],value:requestDetail(4),deferred:false,content:bindings?[{path:['request','effective_system_prompt','text'],content_ref:ref,deferred:true}]:[]};
 server.handlers.set('session/traceDetail',()=>({type:'domain_page',page}));
 server.handlers.set('conversation/contentRead',request=>{if(request.method!=='conversation/contentRead')throw Error('content');return {type:'content_chunk',chunk:{content_ref:request.params.content_ref,offset:0,data_base64:btoa('body'),end_offset:4,readable_end:4,next_offset:null,complete:true}};});
 await server.client.loadTraceDetail('A','trace:4');const first=server.client.getSnapshot().views.A.trace!.details['trace:4'];expect(first.detail).toEqual(requestDetail(4));
 const count=server.requests.filter(row=>row.request.method==='session/traceDetail').length;
 await server.client.refresh('A');const retained=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 if(bindings){
  expect(retained).toBeUndefined();await expect(first.readContent!(ref,0,()=>true)).rejects.toThrow();
  expect(server.requests.filter(row=>row.request.method==='conversation/contentRead')).toHaveLength(0);
  await server.client.loadTraceDetail('A','trace:4');const next=server.client.getSnapshot().views.A.trace!.details['trace:4'];
  expect(next.detail).toEqual(first.detail);expect(await next.readContent!(ref,0,()=>true)).toMatchObject({content_ref:ref,data_base64:btoa('body')});
  expect(server.requests.filter(row=>row.request.method==='session/traceDetail')).toHaveLength(count+1);
 }else{
  expect(retained).toBe(first);await server.client.loadTraceDetail('A','trace:4');expect(server.requests.filter(row=>row.request.method==='session/traceDetail')).toHaveLength(count);
 }
});

it('rejects a selected page cursor that differs from its exact returned read cut before any value read',async()=>{
 const {server}=await root();
 server.handlers.set('conversation/checkpointPage',request=>{const result=fieldPage(request);if(result.type!=='checkpoint_page')throw Error('page');return {...result,page:{...result.page,cursor:'10'}};});
 show(server);
 await waitFor(()=>expect(screen.getByRole('alert').textContent).toContain('Original Trace content page changed'));
 expect(server.client.getSnapshot().views.A.trace!.details['trace:4'].page).toBeDefined();
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
});

it.each(['terminal refresh','owner terminal refresh','explicit reselect'] as const)('a resident child inspector retries a failed detail after %s without rebuilding its reader',async recovery=>{
 const {useSyncExternalStore}=await import('react');
 const server=new Server();servers.push(server);await server.attached('A');
 const cut={...checkpoint(snapshot('child')).read_cut,activation_id:'activation-child'};
 let row:import('../../protocol/app-server/v57').TraceRecord={...record(),state:recovery==='owner terminal refresh'?'completed':'running'};
 server.handlers.set('agent/trace',()=>({type:'trace',page:{read_cut:cut,records:[row],next_cursor:null}}));
 server.held.add('agent/traceDetail');
 const original=nativeTraceFixture(row,requestDetail(4),cut);
 server.handlers.set('conversation/checkpointPage',request=>{if(request.method!=='conversation/checkpointPage')throw Error('page');return {type:'checkpoint_page',page:original.original.page(cut,request.params.domain,request.params.offset,()=>true)};});
 server.handlers.set('conversation/checkpointValueRead',request=>{if(request.method!=='conversation/checkpointValueRead')throw Error('value');return {type:'checkpoint_value_chunk',chunk:original.original.value(cut,request.params,request.params.offset,()=>true)};});
 const reader=new AgentTraceReader(server.client,'A',server.client.getSnapshot().views.A.attachmentObservation!,{agent_id:'child',activation_id:'activation-child',child_conversation_id:cut.conversation_id,state:'active'});
 await reader.refresh();reader.select(row.id);
 function View(){const cache=useSyncExternalStore(reader.subscribe,reader.snapshot);return <Trajectory cache={cache} loadEarlier={()=>{}} onSelect={reader.select} onLoadDetail={reader.detail}/>;}
 render(<View/>);
 const failed=await server.waitFor('agent/traceDetail',1);
 await act(async()=>server.socket.deliver({jsonrpc:'2.0',id:failed.id,error:{code:-32000,message:'native child checkpoint is required for Trace inspection'}}));
 await screen.findByText(/native child checkpoint is required/);
 if(recovery==='terminal refresh'){
  row={...row,state:'completed'};
  await act(async()=>reader.refresh());
 }else if(recovery==='owner terminal refresh'){
  await act(async()=>reader.refresh('inactive'));
 }else{
  await act(async()=>{reader.select(undefined);});await act(async()=>{reader.select(row.id);});
 }
 const retry=await server.waitFor('agent/traceDetail',2);
 await act(async()=>server.socket.success(retry,{type:'domain_page',page:descriptor(cut)}));
 await waitFor(()=>expect(reader.snapshot().details[row.id].error).toBeUndefined());
 expect(reader.snapshot().details[row.id].page).toEqual(descriptor(cut));
 expect(screen.queryByText(/native child checkpoint is required/)).toBeNull();
 // Successful immutable detail is retained over further lifecycle-only refreshes.
 const body=reader.snapshot().details[row.id];await act(async()=>reader.refresh());
 expect(reader.snapshot().details[row.id]).toBe(body);
 expect(server.requests.filter(item=>item.request.method==='agent/traceDetail')).toHaveLength(2);
 reader.retire();
});
