import {act,cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,CheckpointReadCut,TraceRecord,TraceToolDefinitionSelection,TraceInspectionHead} from '../../protocol/app-server/v57';
import {TrajectoryInspector,type TrajectoryInspectorProps} from '../src/app/trajectory/TrajectoryInspector';
import {ArtifactContext} from '../src/app/components/Artifact';
import {PreviewContext} from '../src/app/components/ArtifactPreview';
import {ArtifactResources} from '../src/client/artifacts';
import {traceTool,toolDetail,requestDetail} from './trace-fixture';
import {checkpoint,Server,snapshot} from './fixture';
import {AgentTraceReader} from '../src/client/agent-trace';
import {inlineTraceDetail,type TraceDetailEntry} from '../src/client/trace';
import {traceInspectionHead} from '../../protocol/app-server/trace-inspection';
const servers:Server[]=[],resources:ArtifactResources[]=[];
afterEach(()=>{cleanup();resources.splice(0).forEach(owner=>owner.dispose());servers.splice(0).forEach(server=>server.client.disconnect());vi.restoreAllMocks();vi.unstubAllGlobals();});
const row=()=>{const record=traceTool(295);record.tool!.name='render_image';record.attachments=[{artifact_id:'artifact_1',name:null,mime_type:'image/png',image:true}];return record;};
const props=(record:TraceRecord):TrajectoryInspectorProps=>({item:{type:'RecordRow',record,display_key:'tool-row',owner_record_id:record.id,facet:'overview',label:'Tool',preview:''},facet:'overview',onFacet:vi.fn(),onLoadDetail:vi.fn(),onClose:vi.fn(),records:[record],executions:new Map(),requestNumbers:new Map(),completeHistory:true,onOpen:vi.fn()});
const toolPage=(cut:CheckpointReadCut,record:TraceRecord,definition:TraceToolDefinitionSelection={type:'unavailable'}):CheckpointPage=>{
 const head:TraceInspectionHead={id:record.id,kind:record.kind,record,content:{type:'tool',definition}};
 return {domain:`trace.records.${record.id.slice(6)}.detail`,cursor:cut.revision,read_cut:cut,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:record.id,key:null,value:JSON.parse(JSON.stringify(head)),deferred:true,body_complete:false,content:[]}]};
};
async function setup(agentId?:string){const server=new Server();servers.push(server);await server.attached('A');const owner=new ArtifactResources(server.client,'A',agentId);resources.push(owner);return {server,owner};}
it.each([undefined,'agent-child'])('keeps a native Tool name and protected artifact actions when optional detail fails (child=%s)',async agentId=>{
 const {server,owner}=await setup(agentId),record=row(),intents={openPreview:vi.fn(),download:vi.fn()};
 const create=vi.fn(()=> 'blob:original-artifact');vi.stubGlobal('URL',Object.assign(URL,{createObjectURL:create,revokeObjectURL:vi.fn()}));
 render(<ArtifactContext.Provider value={owner}><PreviewContext.Provider value={intents}><TrajectoryInspector {...props(record)} error="content_source_unavailable"/></PreviewContext.Provider></ArtifactContext.Provider>);
 expect(screen.getByText('render_image')).toBeTruthy();expect(screen.getAllByRole('alert').length).toBeGreaterThan(0);
 const reads=()=>server.requests.filter(entry=>entry.request.method==='artifact/read'||entry.request.method==='agent/artifactRead');
 expect(reads()).toHaveLength(0);expect(screen.queryByText('Run one command.')).toBeNull();
 fireEvent.click(screen.getByRole('button',{name:'Preview artifact_1'}));
 expect(intents.openPreview).toHaveBeenCalledExactlyOnceWith({source:{kind:'artifact',id:'artifact_1'},name:'artifact_1',image:true,mimeType:'image/png'});expect(reads()).toHaveLength(0);
 const method=agentId?'agent/artifactRead':'artifact/read';server.held.add(method);
 fireEvent.click(screen.getByRole('button',{name:'Load attachment'}));const request=await server.waitFor(method,1);
 expect(request.params).toEqual({target:server.client.target('A'),...(agentId?{agent_id:agentId}:{}),artifact_id:'artifact_1'});
 // The existing protected owner still fences a reply after its view is retired.
 owner.dispose();server.socket.success(request,{type:'artifact_bytes',data:'aGk='});await waitFor(()=>expect(screen.getByText(/Obsolete artifact/)).toBeTruthy());expect(create).not.toHaveBeenCalled();
});
it('a deferred finite Tool header retains its positive name and artifacts without pretending to be complete detail',async()=>{
 const {owner}=await setup(),record=row(),cut=checkpoint(snapshot()).read_cut;
 const page=toolPage(cut,record);
 const load=vi.fn();render(<ArtifactContext.Provider value={owner}><PreviewContext.Provider value={{openPreview:vi.fn(),download:vi.fn()}}><TrajectoryInspector {...props(record)} detailPage={page} onLoadDetail={load}/></PreviewContext.Provider></ArtifactContext.Provider>);
 expect(screen.getByText('render_image')).toBeTruthy();expect(screen.getByRole('button',{name:'Preview artifact_1'})).toBeTruthy();expect(screen.queryByRole('alert')).toBeNull();expect(load).not.toHaveBeenCalled();expect(screen.queryByText('Run one command.')).toBeNull();
});
it('complete JSON Tool output retains the same original artifact actions',async()=>{
 const {owner}=await setup(),record=row(),detail=toolDetail(295);detail.tool!.result!.blocks=[{type:'text',text:{text:'{"original":true}',truncated:false}}];
 render(<ArtifactContext.Provider value={owner}><PreviewContext.Provider value={{openPreview:vi.fn(),download:vi.fn()}}><TrajectoryInspector {...props(record)} detail={detail}/></PreviewContext.Provider></ArtifactContext.Provider>);
 expect(screen.getByRole('button',{name:'Preview artifact_1'})).toBeTruthy();expect(screen.queryByRole('alert')).toBeNull();
});
it('missing aliases or artifact facts never produce a fabricated Tool name or actionable artifact',()=>{
 const record=row();record.tool!.name=null;record.attachments=[];
 render(<TrajectoryInspector {...props(record)} error="content_source_unavailable"/>);
 expect(screen.queryByText('render_image')).toBeNull();expect(screen.queryByRole('button',{name:/^Preview /})).toBeNull();
});

const schemaOriginal='{"tool_id":"original-tool","name":"render_image","description":{"text":"ORIGINAL_FROZEN_SCHEMA","truncated":false},"input_schema":{"value":{"opaque":18446744073709551615},"truncated":false}}';
async function selected(kind:'root'|'child'|'historical') {
 const server=new Server();servers.push(server);const record=row();let cut:CheckpointReadCut,entry:TraceDetailEntry,retire:()=>Promise<void>;
 const definition:TraceToolDefinitionSelection={type:'available',field:{type:'value',field:'definition',domain:'trace.records.2.detail.request.tools',item_offset:3,path:[],encoded_bytes:String(new TextEncoder().encode(schemaOriginal).length)}};
 if(kind==='root'){
  server.snapshots.set('A',{...snapshot(),trace:{records:[record],next_cursor:null}});await server.attached('A');
  cut={...checkpoint(snapshot()).read_cut,revision:'9',through:'300'};
  server.handlers.set('session/traceDetail',()=>({type:'domain_page',page:toolPage(cut,record,definition)}));await server.client.loadTraceDetail('A',record.id);
  entry=server.client.getSnapshot().views.A.trace!.details[record.id];server.held.add('session/detach');retire=async()=>{const release=server.client.release('A');const detach=await server.waitFor('session/detach',1);server.reply(detach);await release;await waitFor(()=>expect(server.client.getSnapshot().views.A.attachment).toBe('detached'));};
 }else if(kind==='child'){
  await server.attached('A');cut={...checkpoint(snapshot('child')).read_cut,activation_id:'activation-child',revision:'12',through:'300'};
  server.handlers.set('agent/trace',()=>({type:'trace',page:{read_cut:cut,records:[record],next_cursor:null}}));server.handlers.set('agent/traceDetail',()=>({type:'domain_page',page:toolPage(cut,record,definition)}));
  const reader=new AgentTraceReader(server.client,'A',server.client.getSnapshot().views.A.attachmentObservation!,{agent_id:'agent-child',activation_id:'activation-child',child_conversation_id:cut.conversation_id,state:'active'});
  await reader.refresh();await reader.detail(record.id);entry=reader.snapshot().details[record.id];retire=async()=>reader.retire();
 }else{
  await server.connect();server.held.add('session/attach');cut={...checkpoint(snapshot()).read_cut,revision:'0',through:'300'};
  server.handlers.set('session/traceHistory',()=>({type:'session_trace_history',conversation_id:cut.conversation_id,page:{read_cut:cut,records:[record],next_cursor:null}}));server.handlers.set('session/traceHistoryDetail',()=>({type:'session_trace_history_detail',conversation_id:cut.conversation_id,page:toolPage(cut,record,definition)}));
  const attach=server.client.attach('A'),request=await server.waitFor('session/attach',1);
  await waitFor(()=>expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records).toHaveLength(1));await server.client.loadTraceDetail('A',record.id);
  entry=server.client.getSnapshot().views.A.tracePreview!.cache.details[record.id];retire=async()=>{server.reply(request);await attach;};
 }
 return {server,record,cut,entry,retire};
}
it.each(['root','child','historical'] as const)('%s opens only the original selected frozen Tool definition at its retained cut',async kind=>{
 const {server,record,cut,entry,retire}=await selected(kind);expect(entry.error).toBeUndefined();expect(entry.detail).toBeUndefined();expect(entry.page).toBeTruthy();
 const method=kind==='historical'?'session/traceValueRead':'conversation/checkpointValueRead';
 server.handlers.set(method,request=>{if(request.method!=='conversation/checkpointValueRead'&&request.method!=='session/traceValueRead')throw Error('field method');return {type:'checkpoint_value_chunk',chunk:{cut:request.params.cut,domain:request.params.domain,item_offset:request.params.item_offset,path:request.params.path,offset:0,readable_end:new TextEncoder().encode(schemaOriginal).length,next_offset:null,representation:'canonical_json_v1',bytes_base64:btoa(schemaOriginal)}};});
 render(<TrajectoryInspector {...props(record)} facet="schema" detailPage={entry.page} detailReader={entry.reader}/>);
 await screen.findByText('ORIGINAL_FROZEN_SCHEMA');expect(screen.getByRole('tree',{name:'render_image parameters JSON'})).toBeTruthy();expect(screen.getByText('18446744073709551615')).toBeTruthy();
 const read=server.requests.filter(row=>row.request.method===method);expect(read).toHaveLength(1);expect(read[0].request.params).toMatchObject({cut,domain:'trace.records.2.detail.request.tools',item_offset:3,path:[],offset:0,max_bytes:65536});
 if(kind==='child')expect(read[0].request.params).toMatchObject({agent_id:'agent-child'});expect(document.body.textContent).not.toContain('18446744073709552000');
 expect(server.requests.filter(row=>row.request.method==='session/traceDetail'||row.request.method==='agent/traceDetail'||row.request.method==='session/traceHistoryDetail')).toHaveLength(1);
 if(kind==='historical')await act(retire);
});
it.each(['root','child','historical'] as const)('%s rejects a late frozen-schema reply after its real read owner retires',async kind=>{
 const {server,record,cut,entry,retire}=await selected(kind),method=kind==='historical'?'session/traceValueRead':'conversation/checkpointValueRead';server.held.add(method);
 render(<TrajectoryInspector {...props(record)} facet="schema" detailPage={entry.page} detailReader={entry.reader}/>);const read=await server.waitFor(method,1);
 await act(retire);await act(async()=>server.socket.success(read,{type:'checkpoint_value_chunk',chunk:{cut,domain:'trace.records.2.detail.request.tools',item_offset:3,path:[],offset:0,readable_end:schemaOriginal.length,next_offset:null,representation:'canonical_json_v1',bytes_base64:btoa(schemaOriginal)}}));
 expect(screen.queryByText('ORIGINAL_FROZEN_SCHEMA')).toBeNull();expect(server.requests.filter(row=>row.request.method===method)).toHaveLength(1);
});
it.each(['absent','unavailable'] as const)('keeps proven Schema %s distinct without inventing a selected source',type=>{
 const record=row(),page=toolPage(checkpoint(snapshot()).read_cut,record,{type});render(<TrajectoryInspector {...props(record)} facet="schema" detailPage={page}/>);
 expect(screen.getByText(type==='absent'?'Original request has no matching Tool definition.':'Schema unavailable')).toBeTruthy();expect(screen.queryByRole('button',{name:'Read original value'})).toBeNull();
});
it('does not overwrite current live lifecycle with durable-only content-head state',()=>{
 const record=row();record.state='running';const durable={...record,state:'incomplete' as const};const page=toolPage(checkpoint(snapshot()).read_cut,durable);
 render(<TrajectoryInspector {...props(record)} detailPage={page}/>);expect(screen.getByText('Pending')).toBeTruthy();expect(screen.queryByText('Incomplete')).toBeNull();
});
it('keeps original indexed Request headers separate and rejects a malformed Tool head',()=>{
 const cut=checkpoint(snapshot()).read_cut,record=row(),page=toolPage(cut,record);const requestPage={...page,items:[{...page.items[0],value:{id:record.id,kind:'request',request:{model:'original-model'}}}]};
 expect(traceInspectionHead(requestPage,record.id)).toBeUndefined();expect(traceInspectionHead(page,record.id)?.kind).toBe('tool');
 const malformed=structuredClone(page);malformed.items[0].value={id:record.id,kind:'tool',record:{id:'trace:999'},content:{type:'tool',definition:{type:'unavailable'}}};expect(()=>traceInspectionHead(malformed,record.id)).toThrow('facts changed');
 const wrongDefinition=toolPage(cut,record,{type:'available',field:{type:'value',field:'definition',domain:'trace.records.2.detail.request.tools',item_offset:3,path:[],encoded_bytes:'3'}});const value=wrongDefinition.items[0].value as unknown as TraceInspectionHead;if(value.content.type==='tool'&&value.content.definition.type==='available'&&value.content.definition.field.type==='value')value.content.definition.field={...value.content.definition.field,domain:'current.tools'};
 expect(()=>traceInspectionHead(wrongDefinition,record.id)).toThrow('selector changed');
});
it('Tool Payload reads bounded original RawJsonUtf8 windows without parsing opaque numbers',async()=>{
 const server=new Server();servers.push(server);const record=row(),cut={...checkpoint(snapshot()).read_cut,revision:'9',through:'300'};
 const prefix=' {"padding":"',raw=prefix+'x'.repeat(65535-prefix.length)+'🙂","opaque":18446744073709551615} \n',bytes=new TextEncoder().encode(raw);
 const reference={conversation_id:cut.conversation_id,source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:2},block_index:0},representation:'raw_json_utf8' as const,version:1,content_revision:1,readable_end:bytes.length};
 record.tool!.fields=[{type:'content',field:'arguments',binding:{path:['tool','arguments'],source_path:[],content_ref:reference,deferred:true}}];
 server.snapshots.set('A',{...snapshot(),trace:{records:[record],next_cursor:null}});await server.attached('A');
 server.handlers.set('session/traceDetail',()=>({type:'domain_page',page:toolPage(cut,record,{type:'absent'})}));
 server.handlers.set('conversation/contentRead',request=>{if(request.method!=='conversation/contentRead')throw Error('content');const start=request.params.offset,end=Math.min(start+request.params.max_bytes,bytes.length);return {type:'content_chunk',chunk:{content_ref:reference,offset:start,data_base64:btoa(String.fromCharCode(...bytes.slice(start,end))),end_offset:end,readable_end:bytes.length,next_offset:end<bytes.length?end:null,complete:end===bytes.length}};});
 await server.client.loadTraceDetail('A',record.id);const entry=server.client.getSnapshot().views.A.trace!.details[record.id];
 render(<TrajectoryInspector {...props(record)} facet="input" detailPage={entry.page} detailReader={entry.reader}/>);
 await screen.findByText('18446744073709551615');expect(screen.getByRole('tree',{name:'Payload JSON'})).toBeTruthy();expect(document.body.textContent).toContain('🙂');
 expect(screen.queryByRole('button',{name:/Read content|Read next chunk/})).toBeNull();expect(document.body.textContent).not.toContain('18446744073709552000');
 const reads=server.requests.filter(row=>row.request.method==='conversation/contentRead');expect(reads).toHaveLength(2);expect(reads.map(row=>'offset' in row.request.params?row.request.params.offset:null)).toEqual([0,65536]);
 for(const {request} of reads)expect(request.params).toMatchObject({target:server.client.target('A'),content_ref:reference,max_bytes:65536});
});

it('inline detail accepts only the current complete Request family',()=>{
 const original=checkpoint(snapshot()).read_cut,page:CheckpointPage={domain:'trace.records.4.detail',cursor:original.revision,read_cut:original,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'trace:4',key:null,value:JSON.parse(JSON.stringify(requestDetail(4))),deferred:false,content:[]}]};
 expect(inlineTraceDetail(page,'trace:4')?.request?.model).toBe('historical-model');const missing=structuredClone(page);if(!missing.items[0].value||typeof missing.items[0].value!=='object')throw Error('fixture');delete (missing.items[0].value as Record<string,unknown>).kind;expect(()=>inlineTraceDetail(missing,'trace:4')).toThrow('Original non-Request Trace');
 const obsolete=structuredClone(page);obsolete.items[0].value=JSON.parse(JSON.stringify(toolDetail(4)));expect(()=>inlineTraceDetail(obsolete,'trace:4')).toThrow('Original non-Request Trace');
});
