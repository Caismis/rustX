import {act,cleanup,fireEvent,render,screen,waitFor,within} from '@testing-library/react';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,CheckpointReadCut,MethodResult,Request} from '../../protocol/app-server/v48';
import {Server,checkpoint,snapshot} from './fixture';
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
 server.handlers.set('conversation/checkpointPage',request=>fieldPage(request));server.handlers.set('conversation/checkpointValueRead',request=>valueReply(request));
 await server.attached('A');await server.client.loadTraceDetail('A','trace:4');server.client.selectTrace('A','trace:4');
 return {server,cut};
}
function show(server:Server){const load=vi.fn();render(<Trajectory cache={server.client.getSnapshot().views.A.trace!} loadEarlier={()=>{}} onSelect={()=>{}} onLoadDetail={load}/>);fireEvent.click(screen.getByRole('tab',{name:'Options'}));return load;}
it('retains a positive deferred header and reads selected Options at its own returned cut',async()=>{
 const {server,cut}=await root(),entry=server.client.getSnapshot().views.A.trace!.details['trace:4'];
 expect(entry.page).toEqual(descriptor(cut));expect(entry.detail).toBeUndefined();expect(entry.error).toBeUndefined();
 const load=show(server);expect(screen.getByText(/console-model/)).toBeTruthy();expect(screen.queryByRole('alert')).toBeNull();expect(load).not.toHaveBeenCalled();
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
 fireEvent.click(screen.getByRole('button',{name:'Read original value'}));await screen.findByText('"console-model"');
 const reads=server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead');expect(reads).toHaveLength(1);
 expect(reads[0].request.params).toMatchObject({cut,domain:'trace.records.4.detail.request.model',item_offset:0,path:[],offset:0,max_bytes:65536});
 await server.client.loadTraceDetail('A','trace:4');expect(server.requests.filter(row=>row.request.method==='session/traceDetail')).toHaveLength(1);
});
it('replaces finite original windows without rounding opaque numbers or splitting UTF8 text',async()=>{
 const {server}=await root(),prefix='{"text":"',text=prefix+'x'.repeat(65535-prefix.length)+'🙂","opaque":18446744073709551615}';
 server.handlers.set('conversation/checkpointPage',request=>fieldPage(request,'1'));
 server.handlers.set('conversation/checkpointValueRead',request=>valueReply(request,text));show(server);
 fireEvent.change(screen.getByRole('combobox',{name:'Request options JSON'}),{target:{value:'options'}});
 fireEvent.click(screen.getByRole('button',{name:'Read original value'}));await screen.findByText('options.0');
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
 fireEvent.click(screen.getAllByRole('button',{name:'Read original value'})[1]);
 await screen.findByRole('button',{name:'Read next chunk'});
 const first=[...document.querySelectorAll('pre')].find(el=>el.textContent?.startsWith(prefix))!.textContent!;expect(new TextEncoder().encode(first)).toHaveLength(65535);
 fireEvent.click(screen.getByRole('button',{name:'Read next chunk'}));await screen.findByText('🙂","opaque":18446744073709551615}');
 const last=screen.getByText('🙂","opaque":18446744073709551615}').textContent!;expect(first+last).toBe(text);expect(screen.queryByText(first)).toBeNull();
 const reads=server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead');expect(reads.map(row=>'offset' in row.request.params?row.request.params.offset:null)).toEqual([0,65536]);
 expect(reads.every(row=>'max_bytes' in row.request.params&&row.request.params.max_bytes===65536)).toBe(true);expect(document.body.textContent).not.toContain('18446744073709552000');
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
 server.handlers.set('conversation/checkpointPage',request=>fieldPage(request));
 server.handlers.set('conversation/checkpointValueRead',request=>valueReply(request,'{"text":"ORIGINAL_PRIOR_PROMPT","truncated":false}'));
 render(<Trajectory cache={cache} loadEarlier={()=>{}} onSelect={()=>{}} onLoadDetail={()=>{}}/>);
 fireEvent.click(document.querySelector('[data-display-type="SystemPromptCell"][data-owner="trace:4"]')!);fireEvent.click(screen.getByRole('tab',{name:'Diff'}));fireEvent.click(screen.getByRole('button',{name:'Read original value'}));
 await screen.findByText('{"text":"ORIGINAL_PRIOR_PROMPT","truncated":false}');
 const read=server.requests.find(row=>row.request.method==='conversation/checkpointValueRead')!.request;
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
 fireEvent.click(screen.getByRole('button',{name:'Read original value'}));const request=await server.waitFor('conversation/checkpointPage',1);
 fireEvent.change(screen.getByRole('combobox',{name:'Request options JSON'}),{target:{value:'contributions'}});
 await act(async()=>server.socket.success(request,fieldPage(request)));
 expect(screen.queryByRole('alert')).toBeNull();expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
 expect((screen.getByRole('combobox',{name:'Request options JSON'}) as HTMLSelectElement).value).toBe('contributions');
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
 show(server);fireEvent.click(screen.getByRole('button',{name:'Read original value'}));
 await waitFor(()=>expect(screen.getByRole('alert').textContent).toContain('Original request field page changed'));
 expect(server.client.getSnapshot().views.A.trace!.details['trace:4'].page).toBeDefined();
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(0);
});
