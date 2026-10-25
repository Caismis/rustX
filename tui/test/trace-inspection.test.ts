import {validateCanonicalInspectionPage} from '../../protocol/app-server/trace-inspection.ts';
import assert from 'node:assert/strict';
import { it } from 'node:test';
import type {CheckpointPage,TraceFieldSource,TraceInspectionHead,TraceInspectionMessage,TraceRecord} from '../../protocol/app-server/v57.ts';
import {harness,nextRequest} from './support/app-server-harness.ts';
import {paramsOf} from './support/app-server-peer.ts';
import {SubagentTranscript} from '../src/app-server/subagent-transcript.ts';
import {checkpoint,snapshot,subagent} from './support/fixtures.ts';

const definition:TraceFieldSource={type:'value',field:'definition',domain:'trace.records.3.detail.request.tools',item_offset:2,path:[],encoded_bytes:'38'};
function record(id='trace:9',kind:TraceRecord['kind']='tool'):TraceRecord {
 return {id,position:id,kind,state:'completed',location:{attempt_id:'original-attempt',attempt_id_source:null,step_id:'1',step_id_source:null},timing:{started_at:'2026-10-15T00:00:00Z',ended_at:'2026-10-15T00:00:01Z',duration_ms:'1000'},fields:[],collections:[],calls:[],attachments:[],has_detail:true,truncated:false,
  ...(kind==='tool'?{tool:{name:'present',call_id:'original-call',tool_id:'tool-present',fields:[],started:true,outcome:'success' as const}}:{})};
}
function page(head:TraceInspectionHead,revision='12',conversation=snapshot().conversation_id):CheckpointPage {
 const sequence=head.id.slice('trace:'.length);
 return {domain:`trace.records.${sequence}.detail`,cursor:revision,read_cut:checkpoint({...snapshot(),conversation_id:conversation},revision).read_cut,total:'1',items:[{offset:'0',identity:head.id,value:head,content:[],deferred:true,body_complete:false}],next_offset:null};
}
function toolHead(id='trace:9',source=definition):TraceInspectionHead{return {id,kind:'tool',record:record(id),content:{type:'tool',definition:{type:'available',field:source}}};}

it('T11 explicit finite Tool inspection exposes its original Definition through the existing content command and returned cut',async()=>{
 const h=await harness();
 const showing=h.dispatcher.submit('/observation trace trace:9');
 const request=await nextRequest(h,'session/traceDetail');
 assert.deepEqual(paramsOf(request,'session/traceDetail'),{target:h.target,record_id:'trace:9'});
 const selected=page(toolHead());h.transport.respond(request.id,{type:'domain_page',page:selected});
 const shown=await showing;assert.equal(shown.kind,'inspect');if(shown.kind==='inspect')assert.match(shown.body,/Tool definition: available/);
 assert.equal(h.session.state.cursor,'0','a read never advances Applied');
 const listing=await h.dispatcher.submit('/content');assert.equal(listing.kind,'inspect');if(listing.kind==='inspect')assert.match(listing.body,/trace:9 · definition/);
 // A later unrelated native CP cannot replace this selected original cut.
 h.transport.notify({jsonrpc:'2.0',method:'session/event',params:{target:h.target,registration_id:'harness-registration-1',cursor:'1',event:{type:'checkpoint_advanced',transition:'unrelated-native-cut',checkpoint:checkpoint(snapshot(),'1')}}});
 const reading=h.dispatcher.submit('/content 1');const range=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(paramsOf(range,'conversation/checkpointValueRead'),{target:h.target,agent_id:null,cut:selected.read_cut,domain:definition.domain,item_offset:2,path:[],offset:0,max_bytes:65536});
 const bytes=Buffer.from('{"name":"present","input_schema":null}');assert.equal(bytes.length,38);
 h.transport.respond(range.id,{type:'checkpoint_value_chunk',chunk:{cut:selected.read_cut,domain:definition.domain,item_offset:2,path:[],offset:0,bytes_base64:bytes.toString('base64'),readable_end:bytes.length,next_offset:null,representation:'canonical_json_v1'}});
 const result=await reading;assert.equal(result.kind,'inspect');if(result.kind==='inspect')assert.match(result.body,/"name":"present"/);
 assert.equal(h.session.state.cursor,'1');h.client.close();
});

it('T11 a held old head cannot replace a newer explicit selection and foreign source cuts are rejected',async()=>{
 const h=await harness();const old=h.session.inspectTrace('trace:9').catch(error=>error);
 const oldRequest=await nextRequest(h,'session/traceDetail',0);
 const fresh=h.session.inspectTrace('trace:10');const freshRequest=await nextRequest(h,'session/traceDetail',1);
 const source={...definition,item_offset:3};h.transport.respond(freshRequest.id,{type:'domain_page',page:page(toolHead('trace:10',source),'13')});await fresh;
 h.transport.respond(oldRequest.id,{type:'domain_page',page:page(toolHead())});assert.match(String(await old),/viewport was replaced/);
 assert.deepEqual(h.session.deferredTraceFields.map(row=>row.source),[source]);
 const foreign=h.session.inspectTrace('trace:11');const wrong=await nextRequest(h,'session/traceDetail',2);
 h.transport.respond(wrong.id,{type:'domain_page',page:page(toolHead('trace:11'),'14','conv_01900000-0000-7000-8000-000000000099')});
 await assert.rejects(foreign,/checkpoint identity/);assert.equal(h.session.deferredTraceFields.length,0);assert.equal(h.session.state.cursor,'0');h.client.close();
});

it('T11 a selected Definition range cannot repaint after its attachment is released',async()=>{
 const h=await harness();const inspecting=h.session.inspectTrace('trace:9');const selected=page(toolHead());
 h.transport.respond((await nextRequest(h,'session/traceDetail')).id,{type:'domain_page',page:selected});await inspecting;
 const reading=h.session.readTraceField(h.session.deferredTraceFields[0]!,0);const read=await nextRequest(h,'conversation/checkpointValueRead');
 const detaching=h.session.detach();const detach=await nextRequest(h,'session/detach');h.transport.respond(detach.id,{type:'detached'});await detaching;
 const bytes=Buffer.from('{"name":"present","input_schema":null}');
 h.transport.respond(read.id,{type:'checkpoint_value_chunk',chunk:{cut:selected.read_cut,domain:definition.domain,item_offset:2,path:[],offset:0,bytes_base64:bytes.toString('base64'),readable_end:bytes.length,next_offset:null,representation:'canonical_json_v1'}});
 await assert.rejects(reading,/view was replaced/);assert.equal(h.session.state.cursor,'0');h.client.close();
});

function canonical(position:number):TraceInspectionMessage {
 const ref={conversation_id:snapshot().conversation_id,source:{type:'canonical_message_block' as const,component:'message_metadata' as const,locator:{type:'by_ledger_position' as const,position},block_index:0},representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:2};
 return {locator:{type:'by_ledger_position',position},identity:null,preview:null,source:null,ordinary_input:false,context_kind:null,attachments:[],fields:[{type:'content',field:'message_id',binding:{path:['message_id'],content_ref:ref,deferred:true}}],collections:[]};
}
it('T11 canonical inspection message pages advance without appending history and expose the selected original sources',async()=>{
 const h=await harness();const domain='trace.records.9.detail.messages';
 const head:TraceInspectionHead={id:'trace:9',kind:'assistant',record:record('trace:9','assistant'),content:{type:'messages',page:{domain,total:'33',items:[canonical(1)],next_offset:1}}};
 const inspecting=h.session.inspectTrace(head.id),initial=page(head);h.transport.respond((await nextRequest(h,'session/traceDetail')).id,{type:'domain_page',page:initial});await inspecting;
 const first=h.dispatcher.submit(`/observation ${domain}`),request=await nextRequest(h,'conversation/checkpointPage');
 assert.deepEqual(paramsOf(request,'conversation/checkpointPage').cut,initial.read_cut);
 const firstPage:CheckpointPage={domain,cursor:'12',read_cut:initial.read_cut,total:'33',items:Array.from({length:32},(_,offset)=>({offset:String(offset),identity:String(offset+1),value:canonical(offset+1),content:[],deferred:true})),next_offset:'32'};
 h.transport.respond(request.id,{type:'checkpoint_page',page:firstPage});await first;assert.equal(h.session.deferredTraceFields.length,32);
 const last=h.dispatcher.submit(`/observation ${domain} 32`),next=await nextRequest(h,'conversation/checkpointPage',1);
 assert.equal(paramsOf(next,'conversation/checkpointPage').offset,32);assert.deepEqual(paramsOf(next,'conversation/checkpointPage').cut,initial.read_cut);
 h.transport.respond(next.id,{type:'checkpoint_page',page:{...firstPage,items:[{offset:'32',identity:'33',value:canonical(33),content:[],deferred:true}],next_offset:null}});await last;
 assert.equal(h.session.deferredTraceFields.length,1,'one current bounded message page replaces its predecessor');
 assert.equal(h.session.deferredTraceFields[0]!.source.type,'content');assert.equal(h.session.state.transcript.length,0);assert.equal(h.session.state.cursor,'0');h.client.close();
});

async function childReader(state:'active'|'inactive'='inactive') {
 const agent=subagent('reader','definition',state),h=await harness(snapshot({agents:[agent]}));
 const reader=new SubagentTranscript(h.session,agent.agent_id);await reader.newest();
 const observed=await nextRequest(h,'agent/observe');
 const native=checkpoint(snapshot({conversation_id:agent.child_conversation_id}));native.read_cut.activation_id=agent.activation_id;
 const installed=new Promise<void>(resolve=>{reader.onChange=resolve;});
 h.transport.respond(observed.id,{type:'agent_observe',observation:{type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source:'durable_only',cursor:null,checkpoint:native}});await installed;
 return {...h,reader,agent};
}
function childPage(agent:ReturnType<typeof subagent>,head=toolHead()):CheckpointPage {
 const selected=page(head,'12',agent.child_conversation_id);selected.read_cut.activation_id=agent.activation_id;return selected;
}
it('T11 DurableOnly Child finite Definition reads retain captured Registry Activation, returned cut and parent carrier',async()=>{
 const h=await childReader();const showing=h.reader.inspectTrace('trace:9');const request=await nextRequest(h,'agent/traceDetail');
 assert.deepEqual(paramsOf(request,'agent/traceDetail'),{target:h.target,agent_id:h.agent.agent_id,record_id:'trace:9'});
 const selected=childPage(h.agent);h.transport.respond(request.id,{type:'domain_page',page:selected});await showing;
 assert.equal(h.reader.inspection?.head?.id,'trace:9');assert.equal(h.reader.phase,'durableOnly');
 const ordinal=h.reader.deferred.length+h.reader.deferredJournal.length+h.reader.deferredAttemptIdentities.length;
 const reading=h.reader.loadContent(ordinal);const range=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(paramsOf(range,'conversation/checkpointValueRead'),{target:h.target,agent_id:h.agent.agent_id,cut:selected.read_cut,domain:definition.domain,item_offset:2,path:[],offset:0,max_bytes:65536});
 const bytes=Buffer.from('{"name":"present","input_schema":null}');h.transport.respond(range.id,{type:'checkpoint_value_chunk',chunk:{cut:selected.read_cut,domain:definition.domain,item_offset:2,path:[],offset:0,bytes_base64:bytes.toString('base64'),readable_end:38,next_offset:null,representation:'canonical_json_v1'}});await reading;
 assert.equal(h.reader.content?.text,bytes.toString());assert.equal(h.reader.state?.cursor,'0');assert.equal(h.session.state.cursor,'0');
 assert.equal(h.transport.transportCount('session/traceDetail'),0);assert.equal(h.transport.transportCount('turn/start'),0);
 h.reader.dispose();h.client.close();
});
it('T11 Child selected-head refuses another Activation and held original field cannot repaint a disposed popup',async()=>{
 const h=await childReader();const showing=h.reader.inspectTrace('trace:9'),request=await nextRequest(h,'agent/traceDetail');
 const foreign=childPage(h.agent);foreign.read_cut.activation_id='foreign-activation';h.transport.respond(request.id,{type:'domain_page',page:foreign});await showing;
 assert.match(h.reader.error!,/checkpoint identity/);assert.equal(h.reader.inspection?.head,undefined);
 const fresh=h.reader.inspectTrace('trace:10'),freshRequest=await nextRequest(h,'agent/traceDetail',1),selected=childPage(h.agent,toolHead('trace:10'));
 h.transport.respond(freshRequest.id,{type:'domain_page',page:selected});await fresh;
 const ordinal=h.reader.deferred.length+h.reader.deferredJournal.length+h.reader.deferredAttemptIdentities.length;
 const reading=h.reader.loadContent(ordinal),range=await nextRequest(h,'conversation/checkpointValueRead');h.reader.dispose();
 const bytes=Buffer.from('{"name":"present","input_schema":null}');h.transport.respond(range.id,{type:'checkpoint_value_chunk',chunk:{cut:selected.read_cut,domain:definition.domain,item_offset:2,path:[],offset:0,bytes_base64:bytes.toString('base64'),readable_end:38,next_offset:null,representation:'canonical_json_v1'}});await reading;
 assert.equal(h.reader.content,undefined);assert.equal(h.reader.inspection,undefined);assert.equal(h.session.state.cursor,'0');h.client.close();
});
it('T11 Child original message continuation retains its exact head cut and replaces one finite viewport',async()=>{
 const h=await childReader(),domain='trace.records.9.detail.messages';
 const head:TraceInspectionHead={id:'trace:9',kind:'assistant',record:record('trace:9','assistant'),content:{type:'messages',page:{domain,total:'2',items:[],next_offset:0}}};
 const showing=h.reader.inspectTrace(head.id),request=await nextRequest(h,'agent/traceDetail'),selected=childPage(h.agent,head);h.transport.respond(request.id,{type:'domain_page',page:selected});await showing;
 const reading=h.reader.nextTraceMessages(),range=await nextRequest(h,'conversation/checkpointPage');
 assert.deepEqual(paramsOf(range,'conversation/checkpointPage'),{target:h.target,agent_id:h.agent.agent_id,cut:selected.read_cut,domain,offset:0,limit:32});
 const values=[canonical(1),canonical(2)];for(const value of values)for(const field of value.fields)if(field.type==='content')field.binding.content_ref.conversation_id=h.agent.child_conversation_id;
 h.transport.respond(range.id,{type:'checkpoint_page',page:{domain,cursor:'12',read_cut:selected.read_cut,total:'2',items:values.map((value,index)=>({offset:String(index),identity:String(index+1),value,deferred:true,content:[]})),next_offset:null}});await reading;
 assert.equal(h.reader.traceFields.length,2);assert.equal(h.reader.inspection?.next,undefined);assert.equal(h.reader.state?.transcript.length,0);assert.equal(h.session.state.cursor,'0');h.reader.dispose();h.client.close();
});
it('T11 switching away and back retires an old Root inspection before its late valid head arrives',async()=>{
 const h=await harness(),reading=h.dispatcher.submit('/observation trace trace:9'),request=await nextRequest(h,'session/traceDetail');
 h.dispatcher.setSession(undefined);h.dispatcher.setSession(h.session);
 h.transport.respond(request.id,{type:'domain_page',page:page(toolHead())});
 const result=await reading;assert.equal(result.kind,'transient');if(result.kind==='transient')assert.match(result.text,/viewport was replaced/);
 assert.equal(h.session.deferredTraceFields.length,0);assert.equal(h.session.state.cursor,'0');h.client.close();
});

for(const oldFailure of [false,true])it(`T11 Child selection fences a held A continuation after B's original range (${oldFailure?'failure':'success'})`,async()=>{
 const h=await childReader(),a:TraceFieldSource={...definition,field:'arguments',item_offset:0,encoded_bytes:'8'},b:TraceFieldSource={...definition,field:'result',item_offset:1,encoded_bytes:'8'},head=toolHead();head.record.fields=[a,b];head.content={type:'tool',definition:{type:'absent'}};
 const selecting=h.reader.inspectTrace(head.id),request=await nextRequest(h,'agent/traceDetail'),selected=childPage(h.agent,head);h.transport.respond(request.id,{type:'domain_page',page:selected});await selecting;
 const start=h.reader.deferred.length+h.reader.deferredJournal.length+h.reader.deferredAttemptIdentities.length;
 const chunk=(item:number,offset:number,text:string,end=8)=>({type:'checkpoint_value_chunk' as const,chunk:{cut:selected.read_cut,domain:definition.domain,item_offset:item,path:[],offset,bytes_base64:Buffer.from(text).toString('base64'),readable_end:end,next_offset:offset+text.length<end?offset+text.length:null,representation:'canonical_json_v1' as const}});
 const first=h.reader.loadContent(start),firstRequest=await nextRequest(h,'conversation/checkpointValueRead');h.transport.respond(firstRequest.id,chunk(0,0,'"AAA'));await first;assert.equal(h.reader.content?.next,4);
 const old=h.reader.loadContent(start,4),oldRequest=await nextRequest(h,'conversation/checkpointValueRead',1);
 const fresh=h.reader.loadContent(start+1),freshRequest=await nextRequest(h,'conversation/checkpointValueRead',2);
 // A superseded failure cannot alter the newer selection while B is pending.
 if(oldFailure){h.transport.respond(oldRequest.id,chunk(0,4,'BAD!',9));await old;assert.equal(h.reader.error,undefined);}
 h.transport.respond(freshRequest.id,chunk(1,0,'"BBBBBB"'));await fresh;assert.equal(h.reader.content?.text,'"BBBBBB"');
 if(!oldFailure){h.transport.respond(oldRequest.id,chunk(0,4,'AAA"'));await old;}
 assert.equal(h.reader.content?.text,'"BBBBBB"');assert.equal(h.reader.content?.index,start+1);assert.equal(h.reader.error,undefined);assert.equal(h.session.state.cursor,'0');h.reader.dispose();h.client.close();
});

function blockHead(agent:ReturnType<typeof subagent>):{head:TraceInspectionHead;selected:CheckpointPage;domain:string} {
 const message=canonical(1);message.fields=[];const domain='messages.ledger:1.blocks';message.collections=[{domain,total:'33',loaded:'0',next_offset:'0',revision:'0'}];
 const head:TraceInspectionHead={id:'trace:9',kind:'assistant',record:record('trace:9','assistant'),content:{type:'messages',page:{domain:'trace.records.9.detail.messages',total:'1',items:[message],next_offset:null}}};
 return {head,selected:childPage(agent,head),domain};
}
it('T11 Child canonical block collection pages expose only finite inline metadata and advertised original ContentRefs',async()=>{
 const h=await childReader(),{head,selected,domain}=blockHead(h.agent);
 const selecting=h.reader.inspectTrace(head.id),request=await nextRequest(h,'agent/traceDetail');h.transport.respond(request.id,{type:'domain_page',page:selected});await selecting;
 const loading=h.reader.selectTraceCollection(),pageRequest=await nextRequest(h,'conversation/checkpointPage');
 assert.deepEqual(paramsOf(pageRequest,'conversation/checkpointPage'),{target:h.target,agent_id:h.agent.agent_id,cut:selected.read_cut,domain,offset:0,limit:32});
 const bytes=Buffer.from('ORIGINAL_BODY'),reference={conversation_id:h.agent.child_conversation_id,source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'utf8_text' as const,version:1,content_revision:1,readable_end:bytes.length};
 const first:CheckpointPage={domain,cursor:'12',read_cut:selected.read_cut,total:'33',items:[{offset:'0',identity:'text',value:{type:'text'},content:[{path:['value','text'],content_ref:reference,deferred:true}],deferred:true}],next_offset:'1'};
 h.transport.respond(pageRequest.id,{type:'checkpoint_page',page:first});await loading;assert.equal(h.reader.collectionSources.length,2);
 const index=h.reader.deferred.length+h.reader.deferredJournal.length+h.reader.deferredAttemptIdentities.length+h.reader.traceFields.length;
 await h.reader.loadContent(index);assert.equal(h.reader.content?.text,'{"type":"text"}');assert.equal(h.transport.transportCount('conversation/checkpointValueRead'),0);
 const body=index+1,reading=h.reader.loadContent(body),range=await nextRequest(h,'conversation/contentRead');
 assert.deepEqual(paramsOf(range,'conversation/contentRead'),{target:h.target,agent_id:h.agent.agent_id,content_ref:reference,offset:0,max_bytes:65536});
 h.transport.respond(range.id,{type:'content_chunk',chunk:{content_ref:reference,offset:0,data_base64:bytes.subarray(0,4).toString('base64'),readable_end:bytes.length,end_offset:4,next_offset:4,complete:false}});await reading;
 const continuation=h.reader.loadContent(body,4),nextRange=await nextRequest(h,'conversation/contentRead',1);
 h.transport.respond(nextRange.id,{type:'content_chunk',chunk:{content_ref:reference,offset:4,data_base64:bytes.subarray(4).toString('base64'),readable_end:bytes.length,end_offset:bytes.length,next_offset:null,complete:true}});await continuation;assert.equal(h.reader.content?.text,bytes.subarray(4).toString());
 const paging=h.reader.nextTraceMessages(),next=await nextRequest(h,'conversation/checkpointPage',1);assert.equal(paramsOf(next,'conversation/checkpointPage').offset,1);
 h.transport.respond(next.id,{type:'checkpoint_page',page:{...first,items:Array.from({length:32},(_,index)=>({offset:String(index+1),identity:`block-${index+1}`,value:{deferred:true},content:[],deferred:true})),next_offset:null}});await paging;
 assert.equal(h.reader.collectionSources.length,32);assert.equal(h.reader.inspection?.collection?.items[0]?.offset,'1');assert.equal(h.reader.content,undefined);assert.equal(h.reader.state?.transcript.length,0);assert.equal(h.session.state.cursor,'0');assert.equal(h.transport.transportCount('conversation/checkpointValueRead'),0);h.reader.dispose();h.client.close();
});
it('T11 Child Tool collections retain sparse original block coordinates rather than tool-count ordinals',async()=>{
 const h=await childReader(),{head,selected}=blockHead(h.agent),domain='messages.ledger:1.tools';
 if(head.content.type!=='messages')throw new Error('native message head');head.content.page.items[0]!.collections=[{domain,total:'2',loaded:'0',next_offset:'2',revision:'0'}];
 const selecting=h.reader.inspectTrace(head.id),request=await nextRequest(h,'agent/traceDetail');h.transport.respond(request.id,{type:'domain_page',page:selected});await selecting;
 const loading=h.reader.selectTraceCollection(),first=await nextRequest(h,'conversation/checkpointPage');
 const native:CheckpointPage={domain,cursor:'12',read_cut:selected.read_cut,total:'2',items:[{offset:'2',identity:'original-call-A',value:{call_id:'A'},content:[],deferred:false}],next_offset:'7'};
 h.transport.respond(first.id,{type:'checkpoint_page',page:native});await loading;assert.equal(h.reader.error,undefined);assert.equal(h.reader.inspection?.collection?.items[0]?.offset,'2');
 const paging=h.reader.nextTraceMessages(),next=await nextRequest(h,'conversation/checkpointPage',1);assert.equal(paramsOf(next,'conversation/checkpointPage').offset,7);
 h.transport.respond(next.id,{type:'checkpoint_page',page:{...native,items:[{offset:'7',identity:'original-call-B',value:{call_id:'B'},content:[],deferred:false}],next_offset:null}});await paging;assert.equal(h.reader.error,undefined);assert.equal(h.reader.inspection?.collection?.items[0]?.offset,'7');assert.equal(h.reader.inspection?.collection?.total,'2');assert.equal(h.session.state.cursor,'0');h.reader.dispose();h.client.close();
});
it('T11 a late Child canonical collection page cannot reopen a retired view',async()=>{
 const h=await childReader(),{head,selected,domain}=blockHead(h.agent),selecting=h.reader.inspectTrace(head.id),request=await nextRequest(h,'agent/traceDetail');h.transport.respond(request.id,{type:'domain_page',page:selected});await selecting;
 const loading=h.reader.selectTraceCollection(),pending=await nextRequest(h,'conversation/checkpointPage');h.reader.dispose();h.transport.respond(pending.id,{type:'checkpoint_page',page:{domain,cursor:'12',read_cut:selected.read_cut,total:'33',items:[{offset:'0',identity:'old',value:{deferred:true},content:[],deferred:true}],next_offset:'1'}});await loading;
 assert.equal(h.reader.inspection,undefined);assert.equal(h.reader.content,undefined);assert.equal(h.session.state.cursor,'0');h.client.close();
});

it('T11 generic observation pages reject mismatched cursor and retain the first selected range extent',async()=>{
 const h=await harness(),domain='trace.records.9.detail.request.options',cut=checkpoint(snapshot()).read_cut;
 const pageRead=h.dispatcher.submit(`/observation ${domain}`),bad=await nextRequest(h,'conversation/checkpointPage');const selected:CheckpointPage={domain,cursor:'0',read_cut:cut,total:'1',items:[{offset:'0',identity:'original',value:{deferred:true},content:[],deferred:true}],next_offset:null};
 h.transport.respond(bad.id,{type:'checkpoint_page',page:{...selected,cursor:'1'}});const refused=await pageRead;assert.equal(refused.kind,'transient');
 const proper=h.dispatcher.submit(`/observation ${domain}`),good=await nextRequest(h,'conversation/checkpointPage',1);h.transport.respond(good.id,{type:'checkpoint_page',page:selected});await proper;
 const chunk=(offset:number,text:string,end:number)=>({type:'checkpoint_value_chunk' as const,chunk:{cut,domain,item_offset:0,path:[],offset,bytes_base64:Buffer.from(text).toString('base64'),readable_end:end,next_offset:offset+text.length<end?offset+text.length:null,representation:'utf8_text' as const}});
 const first=h.dispatcher.submit('/observation-value 0 0'),firstRequest=await nextRequest(h,'conversation/checkpointValueRead');h.transport.respond(firstRequest.id,chunk(0,'AB',4));assert.equal((await first).kind,'inspect');
 const growing=h.dispatcher.submit('/observation-value 0 2'),next=await nextRequest(h,'conversation/checkpointValueRead',1);h.transport.respond(next.id,chunk(2,'CD',5));const result=await growing;assert.equal(result.kind,'transient');if(result.kind==='transient')assert.match(result.text,/extent changed/);
 assert.equal(h.session.state.cursor,'0');h.client.close();
});
it('T11 a held generic observation range refuses an away-and-back view after original byte zero',async()=>{
 const h=await harness(),domain='trace.records.9.detail.request.options',cut=checkpoint(snapshot()).read_cut;
 const read=h.dispatcher.submit(`/observation ${domain}`),request=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(request.id,{type:'checkpoint_page',page:{domain,cursor:'0',read_cut:cut,total:'1',items:[{offset:'0',identity:'original',value:{deferred:true},content:[],deferred:true}],next_offset:null}});await read;
 const held=h.dispatcher.submit('/observation-value 0 0'),range=await nextRequest(h,'conversation/checkpointValueRead');h.dispatcher.setSession(undefined);h.dispatcher.setSession(h.session);
 h.transport.respond(range.id,{type:'checkpoint_value_chunk',chunk:{cut,domain,item_offset:0,path:[],offset:0,bytes_base64:Buffer.from('LATE').toString('base64'),readable_end:4,next_offset:null,representation:'utf8_text'}});const result=await held;assert.equal(result.kind,'transient');if(result.kind==='transient')assert.doesNotMatch(result.text,/LATE/);assert.equal(h.session.state.cursor,'0');h.client.close();
});

for(const oldFailure of [false,true])it(`T11 Root byte selection preserves B's split UTF8 decoder after a superseded A ${oldFailure?'failure':'continuation'}`,async()=>{
 const h=await harness(),a:TraceFieldSource={...definition,field:'arguments',item_offset:0,encoded_bytes:'8'},b:TraceFieldSource={...definition,field:'result',item_offset:1,encoded_bytes:'6'},head=toolHead();head.record.fields=[a,b];head.content={type:'tool',definition:{type:'absent'}};
 const showing=h.dispatcher.submit('/observation trace trace:9'),request=await nextRequest(h,'session/traceDetail'),selected=page(head);h.transport.respond(request.id,{type:'domain_page',page:selected});await showing;
 const chunk=(item:number,offset:number,bytes:Buffer,end:number)=>({type:'checkpoint_value_chunk' as const,chunk:{cut:selected.read_cut,domain:definition.domain,item_offset:item,path:[],offset,bytes_base64:bytes.toString('base64'),readable_end:end,next_offset:offset+bytes.length<end?offset+bytes.length:null,representation:'canonical_json_v1' as const}});
 const initial=h.dispatcher.submit('/content 1 0'),initialRequest=await nextRequest(h,'conversation/checkpointValueRead');h.transport.respond(initialRequest.id,chunk(0,0,Buffer.from('"AAA'),8));await initial;
 const old=h.dispatcher.submit('/content 1 4'),oldRequest=await nextRequest(h,'conversation/checkpointValueRead',1);
 const bytes=Buffer.from('"🙂"');assert.equal(bytes.length,6);
 const fresh=h.dispatcher.submit('/content 2 0'),freshRequest=await nextRequest(h,'conversation/checkpointValueRead',2);h.transport.respond(freshRequest.id,chunk(1,0,bytes.subarray(0,3),6));assert.equal((await fresh).kind,'inspect');
 h.transport.respond(oldRequest.id,chunk(0,4,Buffer.from('AAA"'),oldFailure?9:8));const retired=await old;assert.equal(retired.kind,'transient');
 const continued=h.dispatcher.submit('/content 2 3'),next=await nextRequest(h,'conversation/checkpointValueRead',3);h.transport.respond(next.id,chunk(1,3,bytes.subarray(3),6));const result=await continued;
 assert.equal(result.kind,'inspect');if(result.kind==='inspect'){assert.match(result.body,/\n🙂"\n/);assert.doesNotMatch(result.body,/�/);}
 assert.equal(h.session.state.cursor,'0');h.client.close();
});

it('T11 Root canonical pages expose inline metadata and exact advertised bytes at the original retained inspection cut',async()=>{
 const h=await harness(),{head,domain}=blockHead(subagent('reader','definition','inactive'));
 const showing=h.dispatcher.submit('/observation trace trace:9'),detail=await nextRequest(h,'session/traceDetail'),original=page(head);h.transport.respond(detail.id,{type:'domain_page',page:original});await showing;
 const raw=' { "n": 1.0000, "opaque": 18446744073709551615 } ',bytes=Buffer.from(raw),reference={conversation_id:h.target.conversation_id,source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'raw_json_utf8' as const,version:1,content_revision:1,readable_end:bytes.length};
 const listing=h.dispatcher.submit(`/observation ${domain}`),request=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(request.id,{type:'checkpoint_page',page:{domain,cursor:'12',read_cut:original.read_cut,total:'33',items:[{offset:'0',identity:'tool_call',value:{type:'tool_call',id:'native-call'},content:[{path:['value','arguments'],content_ref:reference,deferred:false}],deferred:true}],next_offset:'1'}});const shown=await listing;
 assert.equal(shown.kind,'inspect');if(shown.kind==='inspect'){assert.match(shown.body,/native-call/);assert.doesNotMatch(shown.body,/observation-value/);}
 assert.equal(h.session.observationContent.length,1);assert.equal(h.session.observationContent[0]?.binding.deferred,false,'an exact advertised raw ref remains readable even beside finite inline metadata');
 h.transport.notify({jsonrpc:'2.0',method:'session/event',params:{target:h.target,registration_id:'harness-registration-1',cursor:'1',event:{type:'checkpoint_advanced',transition:'unrelated-owner-publication',checkpoint:checkpoint(snapshot(),'1')}}});
 const reading=h.dispatcher.submit('/content 1'),range=await nextRequest(h,'conversation/contentRead');assert.deepEqual(paramsOf(range,'conversation/contentRead'),{target:h.target,agent_id:null,content_ref:reference,offset:0,max_bytes:65536});
 h.transport.respond(range.id,{type:'content_chunk',chunk:{content_ref:reference,offset:0,data_base64:bytes.toString('base64'),readable_end:bytes.length,end_offset:bytes.length,next_offset:null,complete:true}});const result=await reading;assert.equal(result.kind,'inspect');if(result.kind==='inspect')assert.ok(result.body.includes(raw));
 assert.equal(h.transport.transportCount('conversation/checkpointValueRead'),0);assert.deepEqual(h.session.observationContent[0]?.cut,original.read_cut);assert.equal(h.session.state.cursor,'1');h.client.close();
});
it('T11 Root selected canonical Page retirement rejects late original bytes before dispatcher publication',async()=>{
 const h=await harness(),{head,domain}=blockHead(subagent('reader','definition','inactive'));
 const show=h.dispatcher.submit('/observation trace trace:9'),detail=await nextRequest(h,'session/traceDetail'),original=page(head);h.transport.respond(detail.id,{type:'domain_page',page:original});await show;
 const reference={conversation_id:h.target.conversation_id,source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_ledger_position' as const,position:1},block_index:0},representation:'utf8_text' as const,version:1,content_revision:1,readable_end:4};
 const listing=h.dispatcher.submit(`/observation ${domain}`),request=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(request.id,{type:'checkpoint_page',page:{domain,cursor:'12',read_cut:original.read_cut,total:'33',items:[{offset:'0',identity:'text',value:{type:'text'},content:[{path:['value','text'],content_ref:reference,deferred:true}],deferred:true}],next_offset:'1'}});await listing;
 const reading=h.dispatcher.submit('/content 1'),range=await nextRequest(h,'conversation/contentRead');const replacing=h.dispatcher.submit('/observation trace trace:10'),fresh=await nextRequest(h,'session/traceDetail',1);h.transport.respond(fresh.id,{type:'domain_page',page:page(toolHead('trace:10'),'13')});await replacing;
 h.transport.respond(range.id,{type:'content_chunk',chunk:{content_ref:reference,offset:0,data_base64:Buffer.from('LATE').toString('base64'),readable_end:4,end_offset:4,next_offset:null,complete:true}});const refused=await reading;assert.equal(refused.kind,'transient');if(refused.kind==='transient')assert.doesNotMatch(refused.text,/LATE/);assert.equal(h.session.observationContent.length,0);assert.equal(h.session.state.cursor,'0');h.client.close();
});
it('T11 Root canonical Tool pages validate sparse original block coordinates at a fixed source cut',async()=>{
 const h=await harness(),{head}=blockHead(subagent('reader','definition','inactive')),domain='messages.ledger:1.tools';if(head.content.type!=='messages')throw Error('native messages');head.content.page.items[0]!.collections=[{domain,total:'2',loaded:'0',next_offset:'2',revision:'0'}];
 const show=h.dispatcher.submit('/observation trace trace:9'),detail=await nextRequest(h,'session/traceDetail'),original=page(head);h.transport.respond(detail.id,{type:'domain_page',page:original});await show;
 const loading=h.dispatcher.submit(`/observation ${domain}`),first=await nextRequest(h,'conversation/checkpointPage');const native:CheckpointPage={domain,cursor:'12',read_cut:original.read_cut,total:'2',items:[{offset:'2',identity:'A',value:{call_id:'original-A'},content:[],deferred:true}],next_offset:'7'};h.transport.respond(first.id,{type:'checkpoint_page',page:native});assert.equal((await loading).kind,'inspect');
 const paging=h.dispatcher.submit(`/observation ${domain} 7`),next=await nextRequest(h,'conversation/checkpointPage',1);assert.equal(paramsOf(next,'conversation/checkpointPage').offset,7);assert.deepEqual(paramsOf(next,'conversation/checkpointPage').cut,original.read_cut);h.transport.respond(next.id,{type:'checkpoint_page',page:{...native,items:[{offset:'7',identity:'B',value:{call_id:'original-B'},content:[],deferred:true}],next_offset:null}});const result=await paging;assert.equal(result.kind,'inspect');if(result.kind==='inspect')assert.match(result.body,/original-B/);assert.equal(h.session.state.cursor,'0');h.client.close();
});

it('T11 a held old Root canonical Page cannot replace a newer page in the same native inspection',async()=>{
 const h=await harness(),{head,domain}=blockHead(subagent('reader','definition','inactive')),show=h.dispatcher.submit('/observation trace trace:9'),detail=await nextRequest(h,'session/traceDetail'),original=page(head);h.transport.respond(detail.id,{type:'domain_page',page:original});await show;
 const native=(offset:number):CheckpointPage=>({domain,cursor:'12',read_cut:original.read_cut,total:'33',items:[{offset:String(offset),identity:`block-${offset}`,value:{type:'text'},content:[{path:['value','text'],content_ref:{conversation_id:h.target.conversation_id,source:{type:'canonical_message_block',component:'payload',locator:{type:'by_ledger_position',position:1},block_index:offset},representation:'utf8_text',version:1,content_revision:1,readable_end:4},deferred:true}],deferred:true}],next_offset:String(offset+1)});
 const baseline=h.dispatcher.submit(`/observation ${domain}`),first=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(first.id,{type:'checkpoint_page',page:native(0)});await baseline;
 const old=h.dispatcher.submit(`/observation ${domain}`),oldRequest=await nextRequest(h,'conversation/checkpointPage',1),fresh=h.dispatcher.submit(`/observation ${domain} 1`),freshRequest=await nextRequest(h,'conversation/checkpointPage',2);h.transport.respond(freshRequest.id,{type:'checkpoint_page',page:native(1)});assert.equal((await fresh).kind,'inspect');h.transport.respond(oldRequest.id,{type:'checkpoint_page',page:native(0)});assert.equal((await old).kind,'transient');
 assert.equal(h.session.observationContent.length,1);const source=h.session.observationContent[0]!.binding.content_ref.source;assert.equal(source.type,'canonical_message_block');if(source.type==='canonical_message_block')assert.equal(source.block_index,1);assert.equal(h.session.state.cursor,'0');h.client.close();
});

it('T11 Root rejects an empty held advertised Tool continuation but permits an explicit empty out-of-range page',async()=>{
 const h=await harness(),{head}=blockHead(subagent('reader','definition','inactive')),domain='messages.ledger:1.tools';if(head.content.type!=='messages')throw Error('native messages');head.content.page.items[0]!.collections=[{domain,total:'2',loaded:'0',next_offset:'2',revision:'0'}];
 const showing=h.dispatcher.submit('/observation trace trace:9'),detail=await nextRequest(h,'session/traceDetail'),original=page(head);h.transport.respond(detail.id,{type:'domain_page',page:original});await showing;
 const first:CheckpointPage={domain,cursor:'12',read_cut:original.read_cut,total:'2',items:[{offset:'2',identity:'A',value:{call_id:'A'},content:[],deferred:true}],next_offset:'7'},reading=h.dispatcher.submit(`/observation ${domain}`),request=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(request.id,{type:'checkpoint_page',page:first});await reading;
 const continuation=h.dispatcher.submit(`/observation ${domain} 7`),held=await nextRequest(h,'conversation/checkpointPage',1);assert.deepEqual(paramsOf(held,'conversation/checkpointPage').cut,original.read_cut);h.transport.respond(held.id,{type:'checkpoint_page',page:{...first,items:[],next_offset:null}});const refused=await continuation;assert.equal(refused.kind,'transient');if(refused.kind==='transient')assert.match(refused.text,/continuation did not advance/);
 const manual=h.dispatcher.submit(`/observation ${domain} 99`),outside=await nextRequest(h,'conversation/checkpointPage',2);h.transport.respond(outside.id,{type:'checkpoint_page',page:{...first,items:[],next_offset:null}});assert.equal((await manual).kind,'inspect');assert.equal(h.session.state.cursor,'0');h.client.close();
});
it('T11 Child refuses an empty original advertised Tool continuation and keeps the preceding finite page',async()=>{
 const h=await childReader(),{head,selected}=blockHead(h.agent),domain='messages.ledger:1.tools';if(head.content.type!=='messages')throw Error('native messages');head.content.page.items[0]!.collections=[{domain,total:'2',loaded:'0',next_offset:'2',revision:'0'}];
 const showing=h.reader.inspectTrace(head.id),detail=await nextRequest(h,'agent/traceDetail');h.transport.respond(detail.id,{type:'domain_page',page:selected});await showing;
 const first:CheckpointPage={domain,cursor:'12',read_cut:selected.read_cut,total:'2',items:[{offset:'2',identity:'A',value:{call_id:'A'},content:[],deferred:false}],next_offset:'7'},reading=h.reader.selectTraceCollection(),request=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(request.id,{type:'checkpoint_page',page:first});await reading;
 const paging=h.reader.nextTraceMessages(),held=await nextRequest(h,'conversation/checkpointPage',1);assert.equal(paramsOf(held,'conversation/checkpointPage').offset,7);h.transport.respond(held.id,{type:'checkpoint_page',page:{...first,items:[],next_offset:null}});await paging;assert.match(h.reader.error!,/continuation did not advance/);assert.equal(h.reader.inspection?.collection,first);assert.equal(h.reader.phase,'durableOnly');assert.equal(h.session.state.cursor,'0');h.reader.dispose();h.client.close();
});
it('T11 canonical geometry rejects noncanonical coordinate strings independently of generated wire validation',()=>{
 const native:CheckpointPage={domain:'messages.ledger:1.tools',cursor:'0',read_cut:checkpoint(snapshot()).read_cut,total:'2',items:[{offset:'2',identity:'A',value:null,content:[],deferred:true}],next_offset:'7'};
 for(const offset of ['02','2e0',' 2','0x2'])assert.throws(()=>validateCanonicalInspectionPage({...native,items:[{...native.items[0]!,offset}]},0,'2'),/collection/);
 for(const next_offset of ['07','7e0',' 7','0x7'])assert.throws(()=>validateCanonicalInspectionPage({...native,next_offset},0,'2'),/collection/);
 assert.doesNotThrow(()=>validateCanonicalInspectionPage({...native,items:[],next_offset:null},99,'2'));
});

it('T11 Child Tool heads expose original result blocks independently of metadata and lifecycle settlement',async()=>{
 const h=await childReader(),head=toolHead(),domain='messages.ledger:7.blocks',metadata=Buffer.from('{"role":"tool","result":{"status":{"type":"success"}}}');
 head.content={type:'tool',definition:{type:'absent'}};head.record.collections=[{domain,total:'2',loaded:'0',next_offset:'0',revision:'1'}];
 const ref=(component:'payload'|'message_metadata',block:number,end:number,representation:'canonical_json_v1'|'raw_json_utf8'|'utf8_text')=>({conversation_id:h.agent.child_conversation_id,source:{type:'canonical_message_block' as const,component,locator:{type:'by_ledger_position' as const,position:7},block_index:block},representation,version:1,content_revision:1,readable_end:end});
 const metaRef=ref('message_metadata',0,metadata.length,'canonical_json_v1');head.record.fields=[{type:'content',field:'result',binding:{path:['result'],source_path:['result'],content_ref:metaRef,deferred:true}}];
 const showing=h.reader.inspectTrace(head.id),detail=await nextRequest(h,'agent/traceDetail'),selected=childPage(h.agent,head);h.transport.respond(detail.id,{type:'domain_page',page:selected});await showing;assert.equal(h.reader.inspectionMessage,undefined);assert.deepEqual(h.reader.inspectionCollections,head.record.collections);
 const base=h.reader.deferred.length+h.reader.deferredJournal.length+h.reader.deferredAttemptIdentities.length,meta=h.reader.loadContent(base),metaRequest=await nextRequest(h,'conversation/contentRead');h.transport.respond(metaRequest.id,{type:'content_chunk',chunk:{content_ref:metaRef,offset:0,data_base64:metadata.toString('base64'),readable_end:metadata.length,end_offset:metadata.length,next_offset:null,complete:true}});await meta;assert.equal(h.reader.content?.text,metadata.toString());
 const loading=h.reader.selectTraceCollection(),request=await nextRequest(h,'conversation/checkpointPage');assert.deepEqual(paramsOf(request,'conversation/checkpointPage'),{target:h.target,agent_id:h.agent.agent_id,cut:selected.read_cut,domain,offset:0,limit:32});
 const first=Buffer.from('{"opaque":18446744073709551615,"text":"Tool original payload"}'),firstRef=ref('payload',0,first.length,'raw_json_utf8'),pageOne:CheckpointPage={domain,cursor:'12',read_cut:selected.read_cut,total:'2',items:[{offset:'0',identity:'json',value:{type:'json'},content:[{path:['value','value'],content_ref:firstRef,deferred:false}],deferred:false}],next_offset:'1'};
 h.transport.respond(request.id,{type:'checkpoint_page',page:pageOne});await loading;
 const body=h.reader.loadContent(base+h.reader.traceFields.length+1),bodyRequest=await nextRequest(h,'conversation/contentRead',1);assert.deepEqual(paramsOf(bodyRequest,'conversation/contentRead').content_ref,firstRef);h.transport.respond(bodyRequest.id,{type:'content_chunk',chunk:{content_ref:firstRef,offset:0,data_base64:first.toString('base64'),readable_end:first.length,end_offset:first.length,next_offset:null,complete:true}});await body;assert.equal(h.reader.content?.text,first.toString());assert.match(h.reader.content!.text,/18446744073709551615/);
 const paging=h.reader.nextTraceMessages(),next=await nextRequest(h,'conversation/checkpointPage',1);assert.equal(paramsOf(next,'conversation/checkpointPage').offset,1);
 const second=Buffer.from('second original Tool result block'),secondRef=ref('payload',1,second.length,'utf8_text');h.transport.respond(next.id,{type:'checkpoint_page',page:{...pageOne,items:[{offset:'1',identity:'text',value:{type:'text'},content:[{path:['value','text'],content_ref:secondRef,deferred:true}],deferred:true}],next_offset:null}});await paging;
 const readingSecond=h.reader.loadContent(base+h.reader.traceFields.length+1),secondRequest=await nextRequest(h,'conversation/contentRead',2);h.transport.respond(secondRequest.id,{type:'content_chunk',chunk:{content_ref:secondRef,offset:0,data_base64:second.toString('base64'),readable_end:second.length,end_offset:second.length,next_offset:null,complete:true}});await readingSecond;
 assert.equal(h.reader.content?.text,second.toString());assert.equal(h.reader.inspectionMessage,undefined);assert.equal(h.reader.inspection?.head?.record,head.record);assert.equal(h.reader.phase,'durableOnly');assert.equal(h.reader.state?.attempt,undefined);assert.equal(h.session.state.cursor,'0');assert.equal(h.transport.transportCount('conversation/checkpointValueRead'),0);assert.equal(h.transport.transportCount('turn/start'),0);h.reader.dispose();h.client.close();
});

it('T11 Root Tool result metadata and original image bytes remain independently reachable through native record collections',async()=>{
 const h=await harness(),head=toolHead(),domain='messages.ledger:7.blocks',metadata=Buffer.from('{"role":"tool","result":{"status":{"type":"success"}}}');head.content={type:'tool',definition:{type:'absent'}};head.record.collections=[{domain,total:'1',loaded:'0',next_offset:'0',revision:'1'}];
 const source={type:'canonical_message_block' as const,component:'message_metadata' as const,locator:{type:'by_ledger_position' as const,position:7},block_index:0},metaRef={conversation_id:h.target.conversation_id,source,representation:'canonical_json_v1' as const,version:1,content_revision:1,readable_end:metadata.length};head.record.fields=[{type:'content',field:'result',binding:{path:['result'],source_path:['result'],content_ref:metaRef,deferred:true}}];
 const showing=h.dispatcher.submit('/observation trace trace:9'),detail=await nextRequest(h,'session/traceDetail'),selected=page(head);h.transport.respond(detail.id,{type:'domain_page',page:selected});await showing;assert.ok(h.session.observationCollections.some(row=>row.frontier.domain===domain));
 const readingMeta=h.dispatcher.submit('/content 1'),meta=await nextRequest(h,'conversation/contentRead');h.transport.respond(meta.id,{type:'content_chunk',chunk:{content_ref:metaRef,offset:0,data_base64:metadata.toString('base64'),readable_end:metadata.length,end_offset:metadata.length,next_offset:null,complete:true}});const resultMeta=await readingMeta;assert.equal(resultMeta.kind,'inspect');if(resultMeta.kind==='inspect')assert.ok(resultMeta.body.includes(metadata.toString()));
 const bytes=Buffer.from([137,80,78,71,0,255]),reference={...metaRef,source:{...source,component:'payload' as const},representation:'binary_bytes' as const,readable_end:bytes.length},loading=h.dispatcher.submit(`/observation ${domain}`),request=await nextRequest(h,'conversation/checkpointPage');h.transport.respond(request.id,{type:'checkpoint_page',page:{domain,cursor:'12',read_cut:selected.read_cut,total:'1',items:[{offset:'0',identity:'image',value:{type:'image',mime_type:'image/png'},content:[{path:['value','binary'],content_ref:reference,deferred:true}],deferred:true}],next_offset:null}});await loading;
 const reading=h.dispatcher.submit('/content 2'),payload=await nextRequest(h,'conversation/contentRead',1);assert.deepEqual(paramsOf(payload,'conversation/contentRead').content_ref,reference);h.transport.respond(payload.id,{type:'content_chunk',chunk:{content_ref:reference,offset:0,data_base64:bytes.toString('base64'),readable_end:bytes.length,end_offset:bytes.length,next_offset:null,complete:true}});const result=await reading;assert.equal(result.kind,'inspect');if(result.kind==='inspect'){assert.match(result.body,/Binary bytes \(Base64\)/);assert.ok(result.body.includes(bytes.toString('base64')));}
 assert.equal(h.session.state.cursor,'0');assert.equal(h.transport.transportCount('conversation/checkpointValueRead'),0);assert.equal(h.transport.transportCount('turn/start'),0);h.client.close();
});

it('selected Projection provenance uses its returned cut and rejects a held range after replacement',async()=>{
 const h=await harness();
 const source:TraceFieldSource={type:'value',field:'provenance',domain:'context.projections.1.items.0.provenance',item_offset:0,path:[],encoded_bytes:'2'};
 const detail=(id:string,revision:string):import('../../protocol/app-server/v57.ts').ProjectionDetail=>({
  read_cut:checkpoint(snapshot(),revision).read_cut,
  checkpoint:{checkpoint_id:id,activation_id:'1',base_frontier:'1',activation_ledger:'1',transcript_anchor:'1',origin:{conversation_id:h.target.conversation_id,activation:'1',completed_journal:'1'},destination_installation:null,original_items:0,derived_items:1,summary_preview:null},
  next_item:null,items:[{occurrence:{checkpoint_id:id,item_index:0},content:{request_occurrence:null,role:'user',message_id:null,source:null,provenance:{type:'derived',sources:[],checkpoint_sources:[],transformations:['summary'],truncated:false},provenance_source:source,blocks:[],truncated:true}}],
 });
 const opening=h.dispatcher.submit('/context projection original');const request=await nextRequest(h,'session/projectionDetail');
 const first=detail('original','7');h.transport.respond(request.id,{type:'projection_detail',detail:first});await opening;
 const listing=await h.dispatcher.submit('/content');assert.equal(listing.kind,'inspect');if(listing.kind==='inspect')assert.match(listing.body,/projection:original\/0 · provenance/);
 const reading=h.dispatcher.submit('/content 1');const range=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(paramsOf(range,'conversation/checkpointValueRead'),{target:h.target,agent_id:null,cut:first.read_cut,domain:source.domain,item_offset:0,path:[],offset:0,max_bytes:65536});
 const replacing=h.dispatcher.submit('/context projection replacement');const replacement=await nextRequest(h,'session/projectionDetail',1);
 h.transport.respond(replacement.id,{type:'projection_detail',detail:detail('replacement','9')});await replacing;
 h.transport.respond(range.id,{type:'checkpoint_value_chunk',chunk:{cut:first.read_cut,domain:source.domain,item_offset:0,path:[],offset:0,bytes_base64:'e30=',readable_end:2,next_offset:null,representation:'canonical_json_v1'}});
 const old=await reading;assert.equal(old.kind,'transient');if(old.kind==='transient')assert.match(old.text,/replaced/);
 assert.equal(h.session.state.cursor,'0');assert.equal(h.session.deferredTraceFields[0]?.record,'projection:replacement/0');h.client.close();
});
