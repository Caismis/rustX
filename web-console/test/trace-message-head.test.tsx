import {act,cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,CheckpointReadCut,TraceInspectionHead,TraceInspectionMessage,TraceRecord,ContentRef,ForegroundToolExecution} from '../../protocol/app-server/v57';
import {TrajectoryInspector,type TrajectoryInspectorProps} from '../src/app/trajectory/TrajectoryInspector';
import {readTraceCollection} from '../src/client/trace-content';
import {traceTool} from './trace-fixture';
import {checkpoint,Server,snapshot} from './fixture';
import {AgentTraceReader} from '../src/client/agent-trace';
import {bindTraceDetailReader,type TraceDetailEntry,type TraceFieldReader} from '../src/client/trace';
const servers:Server[]=[];
afterEach(()=>{cleanup();servers.splice(0).forEach(server=>server.client.disconnect());});
const cut=()=>({...checkpoint(snapshot()).read_cut,revision:'9',through:'300'});
function row(index:number):TraceInspectionMessage{return {locator:{type:'by_ledger_position',position:index+1},identity:`message-${index}`,preview:{text:`ORIGINAL_PREVIEW_${index}`,truncated:true},source:'human',ordinary_input:index%2===0,context_kind:null,attachments:[],fields:[],collections:[]};}

const record=(kind:TraceRecord['kind']='user')=>({...traceTool(295),kind,tool:null,state:'completed' as const});
function head(original:CheckpointReadCut,kind:TraceRecord['kind']='user',total=19):CheckpointPage {
 const native=record(kind),value:TraceInspectionHead={id:native.id,kind,record:native,content:{type:'messages',page:{domain:'trace.records.295.detail.messages',total:String(total),items:Array.from({length:Math.min(8,total)},(_,index)=>row(index)),next_offset:total>8?8:null}}};
 return {read_cut:original,cursor:original.revision,domain:'trace.records.295.detail',total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:native.id,key:null,deferred:true,body_complete:false,content:[],value:JSON.parse(JSON.stringify(value))}]};
}
function messagePage(original:CheckpointReadCut,offset:number):CheckpointPage {return {read_cut:original,cursor:original.revision,domain:'trace.records.295.detail.messages',total:'19',next_offset:offset===8?'16':null,collections:[],items:Array.from({length:offset===8?8:3},(_,index)=>({offset:String(offset+index),identity:`ledger:${offset+index+1}`,key:null,value:JSON.parse(JSON.stringify(row(offset+index))),deferred:true,body_complete:false,content:[]}))};}
function props(native:TraceRecord):TrajectoryInspectorProps{return {item:{type:'RecordRow',record:native,display_key:'row',owner_record_id:native.id,facet:'overview',label:'Message',preview:''},facet:'rendered',onFacet:vi.fn(),onLoadDetail:vi.fn(),onClose:vi.fn(),records:[native],executions:new Map(),requestNumbers:new Map(),completeHistory:true,onOpen:vi.fn()};}
function direct(original=cut()) {let active=true;const value=vi.fn(),content=vi.fn(),page=vi.fn(async(_cut,_domain,offset)=>messagePage(original,offset));const source:TraceFieldReader={value,content,page};return {reader:bindTraceDetailReader(source,head(original),()=>active),page,value,content,retire:()=>{active=false;}};}
function advertise(page:CheckpointPage,domain:string,total='33') {
 const head=page.items[0].value as unknown as TraceInspectionHead;if(head.content.type!=='messages')throw Error('fixture');
 head.content.page.items[0].collections=[{domain,revision:'1',total,loaded:'0',next_offset:'0'}];
}
it.each(['assistant','user','context'] as const)('%s retains its native preview without adding source controls to rendered content',kind=>{
 const original=cut(),page=head(original,kind,1),load=vi.fn();render(<TrajectoryInspector {...props(record(kind))} facet="rendered" detailPage={page} onLoadDetail={load}/>);
 expect(screen.getByText('ORIGINAL_PREVIEW_0')).toBeTruthy();expect(load).not.toHaveBeenCalled();expect(screen.queryByRole('button',{name:/Read content|Read original/})).toBeNull();expect(screen.queryByText('Ordinary input: true')).toBeNull();
});
it('compaction preserves its native operation lifecycle without exposing raw identity reads',()=>{
 const original=cut(),native=record('compaction');native.fields=[{type:'value',field:'native_id',domain:'journal.295',item_offset:0,path:['event','operation_id'],encoded_bytes:'20'}];
 const page=head(original,'user',1);page.items[0].value=JSON.parse(JSON.stringify({id:native.id,kind:'compaction',record:native,content:{type:'lifecycle'}}));const reader=direct(original);
 render(<TrajectoryInspector {...props(native)} facet="overview" detailPage={page}/>);
 expect(screen.getByText('Completed')).toBeTruthy();expect(screen.queryByRole('button',{name:/Read original/})).toBeNull();expect(reader.value).not.toHaveBeenCalled();
});
it('message continuation keeps native ordinals and displays the complete batch without ID selection',async()=>{
 const original=cut(),reader=direct(original);render(<TrajectoryInspector {...props(record())} detailPage={head(original)} detailReader={reader.reader}/>);
 await screen.findByText('ORIGINAL_PREVIEW_18');expect(screen.getByText('ORIGINAL_PREVIEW_0')).toBeTruthy();expect(screen.queryByRole('combobox')).toBeNull();
 expect(reader.page.mock.calls.map(args=>args.slice(0,3))).toEqual([[original,'trace.records.295.detail.messages',8],[original,'trace.records.295.detail.messages',16]]);
});
it.each(['cursor','cut','offset','total','empty'] as const)('rejects changed %s on a native message continuation',async fault=>{
 const original=cut(),reader=direct(original),response=messagePage(original,8);
 if(fault==='cursor')response.cursor='10';if(fault==='cut')response.read_cut={...original,revision:'10'};if(fault==='offset')response.items[0].offset='9';if(fault==='total')response.total='20';if(fault==='empty')response.items=[];
 reader.page.mockResolvedValue(response);render(<TrajectoryInspector {...props(record())} detailPage={head(original)} detailReader={reader.reader}/>);await screen.findByRole('alert');expect(screen.queryByText('ORIGINAL_PREVIEW_8')).toBeNull();
});
async function selected(kind:'root'|'child'|'historical',total=1) {
 const server=new Server();servers.push(server);const native=record(),original=kind==='child'?{...cut(),activation_id:'child-activation',conversation_id:'child'}:kind==='historical'?{...cut(),revision:'0'}:cut();let entry:TraceDetailEntry,retire:()=>Promise<void>;
 if(kind==='root'){
  server.snapshots.set('A',{...snapshot(),trace:{records:[native],next_cursor:null}});await server.attached('A');server.handlers.set('session/traceDetail',()=>({type:'domain_page',page:head(original,'user',total)}));await server.client.loadTraceDetail('A',native.id);entry=server.client.getSnapshot().views.A.trace!.details[native.id];retire=async()=>server.client.disconnect();
 }else if(kind==='child'){
  await server.attached('A');server.handlers.set('agent/trace',()=>({type:'trace',page:{read_cut:original,records:[native],next_cursor:null}}));server.handlers.set('agent/traceDetail',()=>({type:'domain_page',page:head(original,'user',total)}));const reader=new AgentTraceReader(server.client,'A',server.client.getSnapshot().views.A.attachmentObservation!,{agent_id:'child',activation_id:'child-activation',child_conversation_id:'child',state:'active'});await reader.refresh();await reader.detail(native.id);entry=reader.snapshot().details[native.id];retire=async()=>reader.retire();
 }else{
  await server.connect();server.held.add('session/attach');server.handlers.set('session/traceHistory',()=>({type:'session_trace_history',conversation_id:original.conversation_id,page:{read_cut:original,records:[native],next_cursor:null}}));server.handlers.set('session/traceHistoryDetail',()=>({type:'session_trace_history_detail',conversation_id:original.conversation_id,page:head(original,'user',total)}));const attach=server.client.attach('A'),request=await server.waitFor('session/attach',1);await waitFor(()=>expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records).toHaveLength(1));await server.client.loadTraceDetail('A',native.id);entry=server.client.getSnapshot().views.A.tracePreview!.cache.details[native.id];retire=async()=>{server.reply(request);await attach;};
 }
 return {server,native,original,entry,retire};
}
it.each(['root','child','historical'] as const)('%s automatically renders original canonical block bytes at the retained cut',async kind=>{
 const {server,native,original,entry,retire}=await selected(kind),domain='messages.ledger:1.blocks',raw=' {"text":"'+'x'.repeat(66000)+'","opaque":18446744073709551615} ',bytes=new TextEncoder().encode(raw),metadata='{"id":"original-call","tool_id":"original-tool","name":"native-tool"}';
 advertise(entry.page!,domain,'1');
 const reference:ContentRef={conversation_id:original.conversation_id,source:{type:'canonical_message_block',component:'payload',locator:{type:'by_ledger_position',position:1},block_index:0},representation:'raw_json_utf8',version:1,content_revision:1,readable_end:bytes.length};
 const metadataRef:ContentRef={...reference,source:{...reference.source,component:'block_metadata'},representation:'canonical_json_v1',readable_end:metadata.length};
 const pageMethod=kind==='historical'?'session/tracePage':'conversation/checkpointPage',method=kind==='historical'?'session/traceContentRead':'conversation/contentRead';
 server.handlers.set(pageMethod,()=>({type:'checkpoint_page',page:{read_cut:original,cursor:original.revision,domain,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'tool_call',key:null,value:null,content:[{path:['value','arguments'],source_path:null,content_ref:reference,deferred:true},{path:['value'],source_path:null,content_ref:metadataRef,deferred:true}],deferred:true,body_complete:null}]}}));
 server.handlers.set(method,request=>{if(request.method!=='session/traceContentRead'&&request.method!=='conversation/contentRead')throw Error('content');const payload=request.params.content_ref.source.component==='block_metadata'?new TextEncoder().encode(metadata):bytes,start=request.params.offset,end=Math.min(start+request.params.max_bytes,payload.length);return {type:'content_chunk',chunk:{content_ref:request.params.content_ref,offset:start,data_base64:btoa(String.fromCharCode(...payload.slice(start,end))),end_offset:end,readable_end:payload.length,next_offset:end<payload.length?end:null,complete:end===payload.length}};});
 render(<TrajectoryInspector {...props(native)} detailPage={entry.page} detailReader={entry.reader}/>);
 await waitFor(()=>expect(document.body.textContent).toContain('18446744073709551615'));expect(document.body.textContent).toContain('native-tool');expect(document.body.textContent).not.toContain('18446744073709552000');
 const requests=server.requests.filter(row=>row.request.method===method);expect(requests.map(row=>'offset' in row.request.params?row.request.params.offset:null)).toEqual([0,0,65536]);
 for(const row of requests)expect(row.request.params).toMatchObject({max_bytes:65536,...kind==='historical'?{cut:original}:{target:server.client.target('A'),agent_id:kind==='child'?'child':null}});
 expect(server.requests.filter(row=>(row.request.method==='session/traceValueRead'||row.request.method==='conversation/checkpointValueRead')&&row.request.params.domain===domain)).toHaveLength(0);if(kind==='historical')await act(retire);
});
it.each(['root','child','historical'] as const)('%s discards a late message page when its native owner retires',async kind=>{
 const {server,native,original,entry,retire}=await selected(kind,19),method=kind==='historical'?'session/tracePage':'conversation/checkpointPage';server.held.add(method);render(<TrajectoryInspector {...props(native)} detailPage={entry.page} detailReader={entry.reader}/>);
 const request=await server.waitFor(method,1);await act(retire);await act(async()=>server.socket.success(request,{type:'checkpoint_page',page:messagePage(original,8)}));expect(screen.queryByText('ORIGINAL_PREVIEW_8')).toBeNull();
});
it('replacing a selected head rejects its pending message page',async()=>{
 const original=cut(),reader=direct(original);let finish!:(page:CheckpointPage)=>void;reader.page.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
 const ui=render(<TrajectoryInspector {...props(record())} detailPage={head(original)} detailReader={reader.reader}/>);const oldFinish=finish,fresh={...original,revision:'10'};
 ui.rerender(<TrajectoryInspector {...props(record())} detailPage={head(fresh)} detailReader={direct(fresh).reader}/>);const stale=messagePage(original,8);(stale.items[0].value as unknown as TraceInspectionMessage).preview={text:'STALE_REPLY',truncated:false};await act(async()=>oldFinish(stale));expect(screen.queryByText('STALE_REPLY')).toBeNull();expect(screen.getByText('ORIGINAL_PREVIEW_0')).toBeTruthy();
});
it('Source retains original generated provenance without exposing content read controls',()=>{
 const original=cut(),page=head(original,'user',1),value=page.items[0].value as unknown as TraceInspectionHead;if(value.content.type!=='messages')throw Error('fixture');value.content.page.items[0].source={agent:{agent_id:'original-agent'}};
 render(<TrajectoryInspector {...props(record())} facet="source" detailPage={page}/>);expect(screen.getByRole('tree',{name:'Message source JSON'})).toBeTruthy();fireEvent.click(screen.getByRole('button',{name:'Expand'}));expect(screen.getAllByText('"original-agent"').length).toBeGreaterThan(0);expect(screen.queryByRole('button',{name:/Read original/})).toBeNull();expect(screen.queryByText('ORIGINAL_PREVIEW_0')).toBeNull();
});
function nativeForeground(block:number):ForegroundToolExecution{return {occurrence:{locator:{type:'by_ledger_position',position:1},block_index:block},message_id:'message-0',block_index:block,call_id:`original-call-${block}`,tool_id:'original-tool',name:`native-tool-${block}`,assembly_complete:true,committed_result_deferred:false,state:{type:'assembled',arguments:'{}'}};}
it('Tool index pages keep sparse native coordinates at the source boundary',async()=>{
 const original=cut(),reader=direct(original),domain='messages.ledger:1.tools';
 reader.page.mockImplementation(async(_cut,_domain,offset)=>{const block=offset===0?2:offset,next=block===2?'7':block===7?'11':null;return {read_cut:original,cursor:original.revision,domain,total:'3',next_offset:next,collections:[],items:[{offset:String(block),identity:`ledger:1:${block}`,key:null,value:JSON.parse(JSON.stringify(nativeForeground(block))),content:[],deferred:false,body_complete:null}]};});
 const items=await readTraceCollection(reader.reader,original,{domain,total:'3'},()=>true);expect(items.map(item=>item.offset)).toEqual(['2','7','11']);
 expect(reader.page.mock.calls.map(args=>args.slice(0,3))).toEqual([[original,domain,0],[original,domain,7],[original,domain,11]]);expect(reader.value).not.toHaveBeenCalled();expect(reader.content).not.toHaveBeenCalled();
});
it.each(['next','duplicate','unsafe','empty'] as const)('rejects malformed sparse Tool %s coordinates',async fault=>{
 const original=cut(),reader=direct(original),domain='messages.ledger:1.tools';
 const item={offset:'2',identity:'ledger:1:2',key:null,value:JSON.parse(JSON.stringify(nativeForeground(2))),content:[],deferred:false,body_complete:null},response:CheckpointPage={read_cut:original,cursor:original.revision,domain,total:'1',next_offset:null,collections:[],items:[item]};
 if(fault==='next')response.next_offset='2';if(fault==='duplicate'){response.total='2';response.items.push({...item});}if(fault==='unsafe')response.items[0].offset='9007199254740992';if(fault==='empty')response.items=[];
 reader.page.mockResolvedValue(response);await expect(readTraceCollection(reader.reader,original,{domain,total:response.total},()=>true)).rejects.toThrow();expect(reader.value).not.toHaveBeenCalled();
});
it('a complete small canonical block preserves its original opaque JSON formatting automatically',async()=>{
 const original=cut(),reader=direct(original),domain='messages.ledger:1.blocks',initial=head(original,'user',1),raw=' {"n":1.0000,"integer":18446744073709551615} ',bytes=new TextEncoder().encode(raw),reference:ContentRef={conversation_id:original.conversation_id,source:{type:'canonical_message_block',component:'payload',locator:{type:'by_ledger_position',position:1},block_index:0},representation:'raw_json_utf8',version:1,content_revision:1,readable_end:bytes.length};advertise(initial,domain,'1');
 reader.page.mockResolvedValue({read_cut:original,cursor:original.revision,domain,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'tool_call',key:null,value:{id:'original-call',tool_id:'original-tool',name:'native-tool',arguments:{n:1}},content:[{path:['value','arguments'],source_path:null,content_ref:reference,deferred:false}],deferred:false,body_complete:null}]});
 reader.content.mockResolvedValue({content_ref:reference,offset:0,data_base64:btoa(raw),end_offset:bytes.length,readable_end:bytes.length,next_offset:null,complete:true});render(<TrajectoryInspector {...props(record())} detailPage={initial} detailReader={reader.reader}/>);await waitFor(()=>expect(document.body.textContent).toContain('18446744073709551615'));expect(document.body.textContent).toContain('1.0000');expect(reader.content.mock.calls[0].slice(0,3)).toEqual([original,reference,0]);expect(reader.value).not.toHaveBeenCalled();
});
