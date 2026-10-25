import { sameToolOccurrence } from '../../protocol/app-server/projection.ts';
import assert from 'node:assert/strict';
import { it } from 'node:test';
import { ObservationChain } from '../../protocol/app-server/observation.ts';
import { ChildObservation } from '../../protocol/app-server/child-observation.ts';
import { ContentCache } from '../../protocol/app-server/content-cache.ts';
import { readEditor } from '../../protocol/app-server/editor.ts';
import type { ContentRef, ContentChunk, EditorLocator, EditorPage, AgentObservation } from '../../protocol/app-server/v57.ts';
import { JsonlDecoder, encodeRecord } from '../src/protocol/jsonl.ts';
import { checkpoint, snapshot } from './support/fixtures.ts';

it('T05 Applied requires complete contiguous successful folds and fences replacement registrations',()=>{
 const chain=new ObservationChain<string>(),folded:string[]=[];
 chain.installCheckpoint('4');chain.beginSubscription('4');
 assert.equal(chain.deliver({registrationId:'old',cursor:'5',event:'stale'},value=>folded.push(value)),'buffered');
 assert.equal(chain.applied,'4');assert.equal(chain.acceptReceipt('fresh','4'),true);assert.equal(folded.length,0);
 assert.equal(chain.deliver({registrationId:'fresh',cursor:'5',event:'first'},value=>folded.push(value)),'applied');
 assert.equal(chain.applied,'5');
 assert.equal(chain.deliver({registrationId:'old',resync:true},()=>assert.fail()),'ignored');
 assert.equal(chain.deliver({registrationId:'fresh',cursor:'6',event:'unfoldable'},()=>{throw new Error('fold failed');}),'recovery');
 assert.equal(chain.applied,'5');assert.equal(chain.phase,'resyncRequired');
 chain.installCheckpoint('5');chain.beginSubscription('5');chain.acceptReceipt('replacement','5');
 assert.equal(chain.deliver({registrationId:'replacement',cursor:'7',event:'gap'},()=>assert.fail()),'recovery');
 assert.equal(chain.applied,'5');assert.deepEqual(folded,['first']);
});
it('T17 real JSONL partial records and disconnect cannot advance Applied or skip reconnect replay',()=>{
 const chain=new ObservationChain<string>();chain.installCheckpoint('9');chain.beginSubscription('9');chain.acceptReceipt('r','9');
 const record=encodeRecord({registrationId:'r',cursor:'10',event:'水🙂'}),prefix=record.subarray(0,record.length-1);
 const partial=new JsonlDecoder();assert.deepEqual(partial.push(prefix),[]);assert.equal(chain.applied,'9');
 assert.throws(()=>partial.finish(),/truncated|unterminated|ended/i);assert.equal(chain.applied,'9');
 const reconnect=new JsonlDecoder(),folded:string[]=[];
 for(const value of reconnect.push(record))chain.deliver(value as {registrationId:string,cursor:string,event:string},event=>folded.push(event));
 assert.equal(chain.applied,'10');assert.deepEqual(folded,['水🙂']);
 // LF is the parsing boundary: no acceptance of an earlier fragment is an ACK.
 const next=encodeRecord({registrationId:'r',cursor:'11',event:'next'}),decoder=new JsonlDecoder();
 assert.deepEqual(decoder.push(next.subarray(0,4)),[]);assert.equal(chain.applied,'10');
 for(const value of decoder.push(next.subarray(4)))chain.deliver(value as {registrationId:string,cursor:string,event:string},event=>folded.push(event));
 assert.equal(chain.applied,'11');assert.deepEqual(folded,['水🙂','next']);
});
it('T20 subscription pending workset is finite and overflow preserves Applied',()=>{
 const chain=new ObservationChain<number>();chain.installCheckpoint('0');chain.beginSubscription('0');
 for(let i=1;i<=32;i++)assert.equal(chain.deliver({registrationId:'r',cursor:String(i),event:i},()=>assert.fail()),'buffered');
 assert.equal(chain.deliver({registrationId:'r',cursor:'33',event:33},()=>assert.fail()),'recovery');
 assert.equal(chain.applied,'0');assert.equal(chain.acceptReceipt('r','0'),false);
});
it('T18 DurableOnly has no Live cursor or execution outcome, and old Activation cannot enter a new view',()=>{
 const conversation=snapshot().conversation_id,child=new ChildObservation('child',conversation,'activation-a');
 const state:AgentObservation={type:'checkpoint',agent_id:'child',conversation_id:conversation,activation_id:'activation-a',source:'durable_only',cursor:null,checkpoint:{...checkpoint(snapshot(),'0'),read_cut:{...checkpoint(snapshot(),'0').read_cut,activation_id:'activation-a'}}};
 child.apply(state);assert.equal(child.phase,'durableOnly');assert.equal(child.cursor,undefined);assert.equal(child.snapshot?.attempt??null,null);
 assert.throws(()=>child.apply({...state,cursor:{agent_id:'child',conversation_id:conversation,activation_id:'activation-a',cursor:'0'}}),/DurableOnly/);
 const successor=new ChildObservation('child',conversation,'activation-b');assert.throws(()=>successor.apply(state),/activation was replaced/);
 const live={...state,checkpoint:{...state.checkpoint,read_cut:{...state.checkpoint.read_cut,activation_id:'activation-b'}},activation_id:'activation-b',source:'live' as const,cursor:{agent_id:'child',conversation_id:conversation,activation_id:'activation-b',cursor:'0'}};
 successor.apply(live);assert.equal(successor.phase,'liveFollowing');
 assert.throws(()=>successor.apply({type:'events',after:{...live.cursor,activation_id:'activation-a'},cursor:{...live.cursor,cursor:'1'},events:[]}),/continuity lost/);
 assert.equal(successor.cursor?.cursor,'0');
});
function reference(length:number):ContentRef{return {conversation_id:snapshot().conversation_id,source:{type:'canonical_message_block',component:'payload',locator:{type:"by_message_id",message_id:'content'},block_index:0},representation:'utf8_text',version:1,content_revision:1,readable_end:length};}
function chunk(ref:ContentRef,offset:number,bytes:Uint8Array):ContentChunk {const end=offset+bytes.length;return {content_ref:ref,offset,data_base64:Buffer.from(bytes).toString('base64'),end_offset:end,readable_end:ref.readable_end,next_offset:end<ref.readable_end?end:null,complete:end===ref.readable_end};}
it('T19 byte cache preserves UTF8 split boundaries and refuses changed source/frontier evidence',async()=>{
 const bytes=Buffer.from('🙂水'),ref=reference(bytes.length);
 const cache=new ContentCache(async(source,offset,max)=>{assert.equal(max,65_536);return chunk(source,offset,bytes.subarray(offset,Math.min(offset+2,bytes.length)));});
 const result:number[]=[];let offset=0;
 for(;;){const read=await cache.read(ref,offset,()=>true);result.push(...read.bytes);if(read.next===undefined)break;offset=read.next;}
 assert.deepEqual(Buffer.from(result),bytes);
 const forged=new ContentCache(async(source,offset)=>({...chunk(source,offset,new Uint8Array([1])),content_ref:{...source,version:2}}));
 await assert.rejects(forged.read(reference(1),0,()=>true),/authoritative content chunk/);
 const nonadvancing=new ContentCache(async(source,offset)=>chunk(source,offset,new Uint8Array()));
 await assert.rejects(nonadvancing.read(reference(1),0,()=>true),/authoritative content chunk/);
});
it('T20 logical view cancellation retains both active read lanes until physical promises terminate',async()=>{
 const gates:((value:ContentChunk)=>void)[]=[],ref=reference(1);let current=true,started=0;
 const cache=new ContentCache(async()=>{started++;return await new Promise(resolve=>gates.push(resolve));});
 const stale=cache.read(ref,0,()=>current).catch(error=>error),live=cache.read(ref,0,()=>true);
 current=false;await assert.rejects(cache.read(ref,0,()=>true),/capacity/);assert.equal(started,2);
 gates[0]!(chunk(ref,0,new Uint8Array([1])));assert.match(String(await stale),/view was replaced/);
 gates[1]!(chunk(ref,0,new Uint8Array([2])));assert.deepEqual((await live).bytes,new Uint8Array([2]));
});
it('T20 four MiB LRU eviction does not alter independently Applied cursor and reloads evicted ranges',async()=>{
 const ref=reference(66*65_536),bytes=new Uint8Array(65_536),chain=new ObservationChain<never>();chain.installCheckpoint('42');let calls=0;
 const cache=new ContentCache(async(source,offset,max)=>{calls++;assert.equal(max,65_536);return chunk(source,offset,bytes);});
 for(let i=0;i<66;i++)await cache.read(ref,i*65_536,()=>true);
 assert.equal(calls,66);assert.equal(chain.applied,'42');await cache.read(ref,0,()=>true);assert.equal(calls,67);assert.equal(chain.applied,'42');
});
it('T19 editor assembles exact split bytes and preserves upload ordering; late reads cannot repaint a replaced view',async()=>{
 const locator:EditorLocator={session_id:'destination',node_id:'node',source_session:'source',source_node:'source-node',source_surface_revision:'1'};
 const bytes=Buffer.from('🙂水'),ref=reference(bytes.length);const reads:number[]=[];
 const result=await readEditor(locator,async(block,offset):Promise<EditorPage>=>{
  reads.push(block);
  if(block===1)return {locator,block_index:block,upload:{session_id:'destination',batch_id:'batch',token:'receipt'},next_block:2};
  if(block===2)return {locator,block_index:block,chunk:chunk(reference(1),0,new Uint8Array([65])),next_block:null};
  return {locator,block_index:block,chunk:chunk(ref,offset,bytes.subarray(offset,Math.min(offset+2,bytes.length))),next_block:1};
 },()=>true);
 assert.deepEqual(result,[{type:'text',text:'🙂水'},{type:'upload',session_id:'destination',batch_id:'batch',token:'receipt'},{type:'text',text:'A'}]);assert.ok(reads.includes(1));
 let release!:(value:EditorPage)=>void,current=true;
 const pending=readEditor(locator,()=>new Promise(resolve=>{release=resolve;}),()=>current).catch(error=>error);
 current=false;release({locator,block_index:0,chunk:chunk(reference(1),0,new Uint8Array([65]))});assert.match(String(await pending),/view was replaced/);
});

it('T15 deferred null fields load through exact versioned byte reads rather than losing authoritative values',async()=>{
 const {materializeFields}=await import('../../protocol/app-server/field-observation.ts');
 const bytes=Buffer.from(JSON.stringify({objective:'水🙂',revision:'18446744073709551615'}));
 const reference={revision:'cut-a',byte_length:bytes.length};let calls=0;
 const value=await materializeFields<{goal:{objective:string;revision:string}},typeof reference>({revision:'cut-a',fields:[{name:'goal',value:null,reference}]},async(ref,offset)=>{
   calls++;const part=bytes.subarray(offset,offset+3),end=offset+part.length;
   return {reference:ref,offset,end_offset:end,data_base64:part.toString('base64'),next_offset:end<bytes.length?end:null};
 },()=>true);
 assert.deepEqual(value.goal,{objective:'水🙂',revision:'18446744073709551615'});assert.ok(calls>1);
});
it('T15 pending domain range materialization preserves exact receipt sequence and revision',async()=>{
 const {domainItems}=await import('../../protocol/app-server/domain-observation.ts');
 const cut=checkpoint(snapshot()).read_cut,bytes=Buffer.from(JSON.stringify({id:'message',content:[{type:'text',text:'水🙂'}]}));
 const page={cursor:'0',read_cut:cut,domain:'inbound.pending',total:'1',next_offset:null,items:[{offset:'91',identity:'message',value:{sequence:'91',revision:'7',message:null},content:[],deferred:true}]};
 const values=await domainItems(page,{current:()=>true,page:async()=>{throw new Error('No page requested');},value:async(page,item,offset)=>{
 const part=bytes.subarray(offset,offset+2),end=offset+part.length;
 return {representation:"canonical_json_v1",cut:page.read_cut,domain:page.domain,item_offset:Number(item.offset),path:[],offset,bytes_base64:part.toString('base64'),readable_end:bytes.length,next_offset:end<bytes.length?end:null};
 }});
 assert.deepEqual(values,[{sequence:'91',revision:'7',message:{id:'message',content:[{type:'text',text:'水🙂'}]}}]);
});

it('T11 T14 deferred native canonical identities render finite receipts without inventing messages or Tool IDs',async()=>{
 const {replaceFromSnapshot}=await import('../src/presentation/projection.ts');
 const {renderTranscript}=await import('../src/ui/components/transcript.ts');
 const {defaultPreferences}=await import('../src/ui/preferences.ts');
 const native=snapshot({message_descriptors:[{identity:null,locator:{type:'by_ledger_position',position:4},role:'assistant',kind:'assistant',block_count:'3',tool_call_count:'1',tool_calls:[],content:[],collections:[]}],attempt:{attempt_id:'a',phase:{type:'running'},turn:0,foreground:[{committed_result_deferred:false,occurrence:{locator:{type:'by_ledger_position',position:4},block_index:2},message_id:null,block_index:2,call_id:null,tool_id:null,name:'',assembly_complete:true,state:{type:'running',arguments:''}}]}});
 const state=replaceFromSnapshot(native,'9');
 const blocks=renderTranscript(state,defaultPreferences());
 assert.equal(state.transcript.length,0);assert.equal(state.cursor,'9');
 assert.ok(blocks.some(block=>block.kind==='text'&&block.text.includes('assistant · 3 blocks · content deferred')));
 assert.ok(blocks.some(block=>block.kind==='text'&&block.text.includes('Tool metadata deferred · running')));
 assert.equal(state.attempt?.foreground[0]?.call_id,null);
});

it('T05 T15 native tool ordinal heads remain finite despite sparse provider block identities',async()=>{
 const {replaceFromSnapshot,reduce}=await import('../src/presentation/projection.ts');
 const {attemptView}=await import('./support/fixtures.ts');
 let state=replaceFromSnapshot(snapshot({attempt:attemptView({attempt_id:'a1'})}),'0');
 state=reduce(state,{cursor:'1',event:{type:'assistant_message_started',attempt_id:'a1',message_id:'m1'}});
 for(let position=0;position<45;position++){
  const block_index=1000-position*7;
  state=reduce(state,{cursor:String(position+2),event:{type:'tool_call_started',position,foreground_position:position,attempt_id:'a1',message_id:'m1',block_index,call:{id:`call-${position}`,tool_id:`tool-${position}`,name:'native Tool'}}});
 }
 const live=state.transcript.find(row=>row.kind==='streaming');
 assert.ok(live?.kind==='streaming');
 assert.equal(live.blocks.length,32);
 assert.deepEqual(live.blocks.map(block=>block.blockIndex),Array.from({length:32},(_,position)=>1000-position*7));
 assert.equal(state.attempt?.foreground.length,32);
 assert.equal(state.cursor,'46');
});

it('T08/T19 deferred Native Tool result reads retain exact source bytes and fence late reads after detach',async()=>{
 const {harness,nextRequest}=await import('./support/app-server-harness.ts');
 const {foreground,attemptView,toolResult}=await import('./support/fixtures.ts');
 const bytes=Buffer.from('{"raw":{"number":9007199254740993},"text":"🙂水"}');
 const ref={sequence:17,path:['event','result'],encoded_bytes:bytes.length};
 const h=await harness(snapshot({attempt:attemptView({foreground:[{...foreground('call','tool','native Tool',{type:'settled',result:toolResult(),arguments:'{}'}),result_source:ref}]})}));
 const first=h.session.readJournalField(ref,0);const read=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(read.params,{target:h.target,agent_id:null,cut:h.session.checkpoint!.read_cut,domain:'journal.17',item_offset:0,path:ref.path,offset:0,max_bytes:65536});
 const split=bytes.length-5;
 const chunk=(offset:number,end:number)=>({representation:'canonical_json_v1' as const,cut:h.session.checkpoint!.read_cut,domain:'journal.17',item_offset:0,path:ref.path,offset,bytes_base64:bytes.subarray(offset,end).toString('base64'),next_offset:end===bytes.length?null:end,readable_end:bytes.length});
 h.transport.respond(read.id,{type:'checkpoint_value_chunk',chunk:chunk(0,split)});const part=await first;assert.deepEqual(part.bytes,bytes.subarray(0,split));assert.equal(part.next,split);
 const second=h.session.readJournalField(ref,split);const continuation=await nextRequest(h,'conversation/checkpointValueRead',1);h.transport.respond(continuation.id,{type:'checkpoint_value_chunk',chunk:chunk(split,bytes.length)});
 assert.deepEqual(Buffer.concat([part.bytes,(await second).bytes]),bytes);assert.equal(h.session.state.cursor,'0');
 const stale=h.session.readJournalField(ref,0);const rejected=assert.rejects(stale,/evidence changed/);const late=await nextRequest(h,'conversation/checkpointValueRead',2);
 const ending=h.session.detach();const detach=await nextRequest(h,'session/detach');h.transport.respond(detach.id,{type:'detached'});await ending;
 h.transport.respond(late.id,{type:'checkpoint_value_chunk',chunk:chunk(0,bytes.length)});await rejected;h.client.close();
});

it('T08 T19 canonical Tool result metadata retains exact original JSON bytes through bounded reads',async()=>{
 const {harness,nextRequest}=await import('./support/app-server-harness.ts');
 const {attemptView,foreground,toolResult}=await import('./support/fixtures.ts');
 const original=Buffer.from('{"result":{"large":9007199254740993,"text":"🙂水"},"arguments":"{ \\"raw\\": 1 }"}');
 const source:ContentRef={...reference(original.length),source:{type:'canonical_message_block',component:'block_metadata',locator:{type:'by_ledger_position',position:7},block_index:0},representation:'canonical_json_v1'};
 const initial=snapshot({attempt:attemptView({foreground:[{...foreground('call','tool','native Tool',{type:'settled',result:toolResult(),arguments:'{}'}),result_metadata_ref:source,committed_result_deferred:true}]})});
 const h=await harness(initial);
 const cut=checkpoint(initial,'1');
 cut.content=[{path:['attempt','foreground','0','state','result'],source_path:['result'],content_ref:source,deferred:true}];
 h.transport.emit(h.target,'1',{type:'checkpoint_advanced',transition:'canonical-tool-metadata',checkpoint:cut});
 assert.deepEqual(h.session.deferredContent.map(binding=>binding.content_ref),[source]);
 const {renderTranscript}=await import('../src/ui/components/transcript.ts');
 const {defaultPreferences}=await import('../src/ui/preferences.ts');
 assert.ok(renderTranscript(h.session.state,defaultPreferences()).some(block=>block.kind==='text'&&block.text.includes('Tool result metadata deferred')));
 const cache=new ContentCache((ref,offset)=>h.session.readContent(ref,offset));
 const reading=cache.read(source,0,()=>true);
 const request=await nextRequest(h,'conversation/contentRead');
 assert.deepEqual(request.params,{target:h.target,agent_id:null,content_ref:source,offset:0,max_bytes:65536});
 h.transport.respond(request.id,{type:'content_chunk',chunk:chunk(source,0,original)});
 const result=await reading;
 assert.deepEqual(Buffer.from(result.bytes),original);
 assert.equal(result.next,undefined);
 assert.equal(h.session.state.cursor,'1','content cache loading does not advance Applied');
 h.client.close();
});


it('T08 T19 terminal failure and finish reason expose their exact indexed public bytes without changing Applied', async () => {
  const {harness,nextRequest}=await import('./support/app-server-harness.ts');
  const {attemptView}=await import('./support/fixtures.ts');
  const bytes=Buffer.from('{"message":"🙂水","retry_after_ms":"9007199254740993"}');
  for (const failed of [true,false]) {
    const reference={sequence:23,path:[failed?'normalized_failure':'finish_reason'],encoded_bytes:bytes.length};
    const outcome=failed
      ? {type:'failed' as const,error:{type:'model' as const,kind:'provider_error' as const,message:'deferred',retry_after_ms:null},error_deferred:true,error_source:reference}
      : {type:'completed' as const,finish_reason:{type:'other' as const,reason:'deferred'},finish_reason_deferred:true,finish_reason_source:reference};
    const h=await harness(snapshot({attempt:attemptView({phase:{type:'settled',outcome}})}));
    assert.equal(h.session.deferredJournalFields[0]?.label,failed?'Attempt failure':'Finish reason');
    const reading=h.session.readJournalField(reference,0);
    const request=await nextRequest(h,'conversation/checkpointValueRead');
    assert.deepEqual(request.params,{target:h.target,agent_id:null,cut:h.session.checkpoint!.read_cut,domain:'journal.23',item_offset:0,path:reference.path,offset:0,max_bytes:65536});
    h.transport.respond(request.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut:h.session.checkpoint!.read_cut,domain:'journal.23',item_offset:0,path:reference.path,offset:0,bytes_base64:bytes.toString('base64'),next_offset:null,readable_end:bytes.length}});
    assert.deepEqual((await reading).bytes,bytes);
    assert.equal(h.session.state.cursor,'0');
    h.client.close();
  }
});

it('a deferred native turn clock reads its original attempt identity without inventing an Attempt or moving Applied',async()=>{
 const {harness,nextRequest}=await import('./support/app-server-harness.ts');
 const bytes=Buffer.from('"'+'original\\u0061-attempt'.repeat(40)+'"'),reference={sequence:7,path:['attempt_id'],encoded_bytes:bytes.length};
 const statistics={turns:'1',steps:'1',completed_responses:'0',model_requests:'1',requests_with_usage:'0',
   latest_turn:{attempt_id:null,attempt_id_deferred:true,attempt_id_source:reference,started_at:'2026-10-14T00:00:00Z'}};
 const h=await harness(snapshot({transcript:{entries:[],statistics}}));
 assert.equal(h.session.deferredJournalFields[0]?.label,'Turn attempt identity');
 assert.equal(h.session.state.attempt,undefined);
 const reading=h.session.readJournalField(reference,0),request=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(request.params,{target:h.target,agent_id:null,cut:h.session.checkpoint!.read_cut,domain:'journal.7',item_offset:0,path:reference.path,offset:0,max_bytes:65536});
 h.transport.respond(request.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut:h.session.checkpoint!.read_cut,domain:'journal.7',item_offset:0,path:reference.path,offset:0,bytes_base64:bytes.toString('base64'),next_offset:null,readable_end:bytes.length}});
 assert.deepEqual((await reading).bytes,bytes);
 assert.equal(h.session.state.cursor,'0');
 assert.equal(h.session.state.statistics?.latest_turn?.attempt_id,null);
 h.client.close();
});

it('native completed process retains its final metadata source while canonical content alone exposes no terminal identity',async()=>{
 const {deferredContent,replaceFromSnapshot}=await import('../src/presentation/projection.ts');
 const base=snapshot(),reference:ContentRef={conversation_id:base.conversation_id,source:{type:'canonical_message_block',component:'message_metadata',block_index:0,locator:{type:'by_ledger_position',position:17}},representation:'canonical_json_v1',version:1,content_revision:1,readable_end:900};
 const process={attempt_id_source:null,conversation_id:base.conversation_id,attempt_id:'a',outcome:'completed' as const,control_cursor:'1',message_count:2,tool_call_count:0,
   final_message_id:null,final_message_locator:{type:'by_ledger_position' as const,position:17},final_message_id_deferred:true,final_message_id_source:reference};
 const entry={cursor:'1',turn_process:process,item:{type:'message' as const,message:{role:'assistant' as const,id:'member',content:[{type:'text' as const,text:'native member'}]}}};
 const completed=replaceFromSnapshot({...base,transcript:{entries:[entry]}},'0');
 assert.deepEqual(deferredContent(completed).map(binding=>binding.content_ref),[reference]);
 assert.equal(completed.transcript[0]?.kind,'committed');
 assert.equal(completed.cursor,'0');
 const running=replaceFromSnapshot({...base,transcript:{entries:[{...entry,turn_process:{...process,outcome:'running'}}]}},'0');
 assert.deepEqual(deferredContent(running),[]);
 assert.equal(running.attempt,undefined);
});

it('T11 keeps deferred canonical Context metadata out of human chat rows', async () => {
 const {replaceFromSnapshot}=await import('../src/presentation/projection.ts');
 const {renderTranscript}=await import('../src/ui/components/transcript.ts');
 const {defaultPreferences}=await import('../src/ui/preferences.ts');
 const descriptor={identity:null,locator:{type:'by_ledger_position' as const,position:17},role:'user',kind:'context/agent_status',block_count:'1',tool_call_count:'0',tool_calls:[],content:[],collections:[]};
 const state=replaceFromSnapshot(snapshot({message_descriptors:[descriptor]}),'0');
 assert.ok(!renderTranscript(state,defaultPreferences()).some(block=>block.key===`canonical:${JSON.stringify(descriptor.locator)}`));
 assert.deepEqual(state.messageDescriptors,[descriptor]);
});

it('T11 T14 deferred Tool Message IDs never alias distinct native occurrences',()=>{
 const first={message_id:null,call_id:null,tool_id:null,occurrence:{locator:{type:'by_ledger_position' as const,position:7},block_index:0}};
 const other={...first,occurrence:{...first.occurrence,locator:{type:'by_ledger_position' as const,position:8}}};
 assert.equal(sameToolOccurrence(first,other),false);
 assert.equal(sameToolOccurrence(first,{...first,occurrence:{...first.occurrence,block_index:1}}),false);
 // Source structural JSON can order these two named fields differently.
 assert.equal(sameToolOccurrence(first,{...first,occurrence:{block_index:0,locator:{position:7,type:'by_ledger_position' as const}}}),true);
});


it('T14 T18 Child native event metadata must retain exact activation before Applied advances',()=>{
 const state=snapshot(),baseline=checkpoint(state,'4'); baseline.read_cut.activation_id='actual-a';
 const identity={agent_id:'child',conversation_id:state.conversation_id,activation_id:'actual-a'};
 for(const activation of [null,'obsolete-a']) {
  const child=new ChildObservation(identity.agent_id,identity.conversation_id,identity.activation_id);
  child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'4'},checkpoint:baseline});
  assert.throws(()=>child.apply({type:'events',after:{...identity,cursor:'4'},cursor:{...identity,cursor:'5'},events:[{cursor:'5',event:{type:'projection_transition',events:[],metadata:{read_cut:{...baseline.read_cut,revision:'5',activation_id:activation},collections:[],content:[]}}}]}),/Child event cut evidence mismatch/);
  assert.equal(child.appliedCursor?.cursor,'4');assert.equal(child.checkpoint?.read_cut.activation_id,'actual-a');
  assert.equal(child.phase,'resyncRequired');
 }
 const child=new ChildObservation(identity.agent_id,identity.conversation_id,identity.activation_id);
 child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'4'},checkpoint:baseline});
 child.apply({type:'events',after:{...identity,cursor:'4'},cursor:{...identity,cursor:'5'},events:[{cursor:'5',event:{type:'projection_transition',events:[],metadata:{read_cut:{...baseline.read_cut,revision:'5'},collections:[],content:[]}}}]});
 assert.equal(child.appliedCursor?.cursor,'5');assert.equal(child.checkpoint?.read_cut.activation_id,'actual-a');
});

it('T14 native C and J refine every loaded Tool cache by occurrence without aliasing reused provider IDs',async()=>{
 const {foldRuntimeEvent}=await import('../../protocol/app-server/projection.ts');
 const make=(position:number,message:string)=>({committed_result_deferred:false,occurrence:{locator:{type:'by_ledger_position' as const,position},block_index:0},message_id:message,block_index:0,call_id:'reused-provider-id',tool_id:'tool',name:'native Tool',assembly_complete:true,state:{type:'assembled' as const,arguments:'{"original":true}'}});
 const old=make(7,'old'),current=make(8,'current');
 let native=snapshot({attempt:{attempt_id:'current-attempt',phase:{type:'running'},turn:0,foreground:[current]},
   message_descriptors:[old,current].map(tool=>({identity:tool.message_id,locator:tool.occurrence.locator,role:'assistant',kind:'assistant',block_count:'1',tool_call_count:'1',tool_calls:[tool],content:[],collections:[]})),
   transcript:{entries:[old,current].map((tool,index)=>({cursor:String(index+1),tool_calls:[tool],item:{type:'message' as const,message:{role:'assistant' as const,id:tool.message_id,content:[{type:'tool_call' as const,id:tool.call_id,tool_id:tool.tool_id,name:tool.name,arguments:{original:true}}]}}}))},
 });
 const result={status:{type:'success' as const},duration_ms:'1',content:[]};
 native=foldRuntimeEvent(native,{result_deferred:false,type:'tool_result_committed',attempt_id:'old-attempt',occurrence:old.occurrence,result});
 for(const slots of [native.transcript.entries![0]!.tool_calls!,native.message_descriptors[0]!.tool_calls]){
  assert.equal(slots[0]!.state.type,'assembled');assert.deepEqual(slots[0]!.committed_result,result);
 }
 assert.equal(native.attempt!.foreground![0]!.committed_result,undefined);
 native=foldRuntimeEvent(native,{type:'tool_execution_started',attempt_id:'old-attempt',occurrence:old.occurrence,tool_call_id:old.call_id,tool_id:old.tool_id});
 assert.equal(native.transcript.entries![0]!.tool_calls![0]!.state.type,'running');
 assert.equal(native.message_descriptors[0]!.tool_calls[0]!.state.type,'running');
 assert.equal(native.transcript.entries![1]!.tool_calls![0]!.state.type,'assembled');
 native=foldRuntimeEvent(native,{type:'tool_execution_settled',attempt_id:'old-attempt',occurrence:old.occurrence,tool_call_id:old.call_id,result});
 assert.equal(native.transcript.entries![0]!.tool_calls![0]!.state.type,'settled');
 assert.equal(native.message_descriptors[0]!.tool_calls[0]!.state.type,'settled');
 assert.equal(native.attempt!.foreground![0]!.state.type,'assembled');
});

it('T20 oversized encoded ranges are rejected before invoking a decoder',async()=>{
 const {domainItems}=await import('../../protocol/app-server/domain-observation.ts');
 const {materializeFields}=await import('../../protocol/app-server/field-observation.ts');
 const {materializeSettings}=await import('../../protocol/app-server/settings-observation.ts');
 const cut=checkpoint(snapshot()).read_cut,encoded='AAAA'.repeat(22_000);
 const original=globalThis.atob;let calls=0;
 globalThis.atob=()=>{calls++;throw new Error('Decoder must not run');};
 try{
  const cache=new ContentCache(async(source,offset)=>({...chunk(source,offset,new Uint8Array([1])),data_base64:encoded}));
  await assert.rejects(cache.read(reference(1),0,()=>true),/decoded-byte bound/);
  const page={cursor:'0',read_cut:cut,domain:'model',total:'1',items:[{offset:'0',identity:'model:0',value:null,content:[],deferred:true}],next_offset:null};
  await assert.rejects(domainItems(page,{current:()=>true,page:async()=>page,value:async()=>({representation:'canonical_json_v1',cut,domain:'model',item_offset:0,path:[],offset:0,readable_end:1,bytes_base64:encoded,next_offset:null})}),/Encoded native range/);
  const ref={revision:'1',byte_length:1};
  await assert.rejects(materializeFields({revision:'1',fields:[{name:'field',reference:ref}]},async()=>({reference:ref,offset:0,end_offset:1,data_base64:encoded,next_offset:null}),()=>true),/Encoded native range/);
  await assert.rejects(materializeSettings({revision:'1',cwd:'/workspace',reference:ref},async(reference)=>({reference,offset:0,end_offset:1,data_base64:encoded,next_offset:null}),()=>true),/Encoded native range/);
  assert.equal(calls,0);
 }finally{globalThis.atob=original;}
});


it('T14 deferred native descriptor process keeps exact completion metadata without creating a transcript body',async()=>{
 const {deferredContent,replaceFromSnapshot}=await import('../src/presentation/projection.ts');
 const {renderTranscript}=await import('../src/ui/components/transcript.ts');
 const {defaultPreferences}=await import('../src/ui/preferences.ts');
 const base=snapshot(),locator={type:'by_ledger_position' as const,position:17};
 const reference:ContentRef={conversation_id:base.conversation_id,source:{type:'canonical_message_block',component:'message_metadata',block_index:0,locator},representation:'canonical_json_v1',version:1,content_revision:1,readable_end:900};
 const owner={attempt_id_source:null,conversation_id:base.conversation_id,attempt_id:'original-attempt',outcome:'completed' as const,control_cursor:'22',message_count:2,tool_call_count:0,
   final_message_id:null,final_message_locator:locator,final_message_id_deferred:true,final_message_id_source:reference};
 const descriptor={identity:null,locator,role:'assistant',kind:'message',block_count:'1',tool_call_count:'0',tool_calls:[],turn_process:owner,completed_response:null,content:[],collections:[]};
 const page={entries:[],deferred_entries:[{offset:'21',identity:'native-ledger:17',value:descriptor,content:[],deferred:true}]};
 const original=JSON.stringify(page);
 const state=replaceFromSnapshot({...base,message_descriptors:[],transcript:page},'0');
 assert.equal(state.transcript.length,0);assert.equal(state.attempt,undefined);assert.equal(state.cursor,'0');
 assert.deepEqual(deferredContent(state).map(binding=>binding.content_ref),[reference]);
 assert.ok(renderTranscript(state,defaultPreferences()).some(block=>block.kind==='text'&&block.text.includes('Turn completed · 2 messages · 0 tools')));
 const incomplete=replaceFromSnapshot({...base,message_descriptors:[],transcript:{...page,deferred_entries:[{...page.deferred_entries[0]!,value:{...descriptor,turn_process:{...owner,outcome:'incomplete_at_cut'}}}]}},'0');
 assert.deepEqual(deferredContent(incomplete),[]);
 assert.ok(renderTranscript(incomplete,defaultPreferences()).some(block=>block.kind==='text'&&block.text.includes('Turn incomplete_at_cut')));
 assert.equal(JSON.stringify(page),original);
});


it('T14 deferred process and response owners share only exact selected native identity, not null or transcript position',async()=>{
 const {attemptOwnerKey}=await import('../../protocol/app-server/attempt-identity.ts');
 const {deferredAttemptIdentities}=await import('../src/presentation/message-descriptors.ts');
 const {replaceFromSnapshot}=await import('../src/presentation/projection.ts');
 const base=snapshot(),source={type:'inherited' as const,kind:'turn' as const,ordinal:0,domain:'transcript.turns.0.identity',encoded_bytes:9000};
 const owner={conversation_id:'original-owner',attempt_id:null,attempt_id_source:source,outcome:'incomplete_at_cut' as const,control_cursor:'1',message_count:2,tool_call_count:0,
   final_message_id:null,final_message_locator:null,final_message_id_deferred:false,final_message_id_source:null};
 const rows=['1','2'].map(cursor=>({cursor,turn_process:owner,item:{type:'message' as const,message:{role:'assistant' as const,id:`member-${cursor}`,content:[]}}}));
 const state=replaceFromSnapshot({...base,transcript:{entries:rows}},'0');
 assert.deepEqual(deferredAttemptIdentities(state),[{owner:attemptOwnerKey(owner),source}]);
 const other={...owner,attempt_id_source:{...source,kind:'response' as const,domain:'transcript.responses.0.identity'}};
 assert.notEqual(attemptOwnerKey(other),attemptOwnerKey(owner));
 assert.throws(()=>attemptOwnerKey({...owner,attempt_id_source:null}),/Native attempt owner identity is unavailable/);
 assert.equal(state.attempt,undefined);assert.equal(state.cursor,'0');
});

it('T08/T14 selected inherited owner identity reads original bytes at its captured cut and fences detach',async()=>{
 const {harness,nextRequest}=await import('./support/app-server-harness.ts');
 const bytes=Buffer.from('"'+'original\\u0061-owner'.repeat(40)+'"');
 const source={type:'inherited' as const,kind:'turn' as const,ordinal:0,domain:'transcript.turns.0.identity',encoded_bytes:bytes.length};
 const owner={conversation_id:'conv_01900000-0000-7000-8000-000000000003',attempt_id:null,attempt_id_source:source,outcome:'incomplete_at_cut' as const,control_cursor:'1',message_count:1,tool_call_count:0,
   final_message_id:null,final_message_locator:null,final_message_id_deferred:false,final_message_id_source:null};
 const initial=snapshot({transcript:{entries:[{cursor:'1',turn_process:owner,item:{type:'message',message:{role:'assistant',id:'native-member',content:[]}}}]}});
 const h=await harness(initial),cut=h.session.checkpoint!.read_cut,split=5;
 const chunk=(offset:number,end:number)=>({cut,domain:source.domain,item_offset:0,path:['attempt_id'],representation:'canonical_json_v1' as const,offset,bytes_base64:bytes.subarray(offset,end).toString('base64'),next_offset:end<bytes.length?end:null,readable_end:bytes.length});
 const first=h.session.readAttemptIdentity(source,0),request=await nextRequest(h,'conversation/checkpointValueRead');
 assert.deepEqual(request.params,{target:h.target,agent_id:null,cut,domain:source.domain,item_offset:0,path:['attempt_id'],offset:0,max_bytes:65536});
 h.transport.respond(request.id,{type:'checkpoint_value_chunk',chunk:chunk(0,split)});const start=await first;
 h.transport.emit(h.target,'1',{type:'checkpoint_advanced',transition:'unrelated-native-publication',checkpoint:checkpoint(initial,'1')});
 const continuation=h.session.readAttemptIdentity(source,split,start.cut),next=await nextRequest(h,'conversation/checkpointValueRead',1);
 assert.equal(next.method,'conversation/checkpointValueRead');assert.ok('cut' in next.params);
 assert.deepEqual(next.params.cut,cut);h.transport.respond(next.id,{type:'checkpoint_value_chunk',chunk:chunk(split,bytes.length)});
 assert.deepEqual(Buffer.concat([start.bytes,(await continuation).bytes]),bytes);
 assert.equal(h.session.state.cursor,'1');assert.equal(h.session.state.attempt,undefined);
 const stale=h.session.readAttemptIdentity(source,0),rejected=assert.rejects(stale,/Native owner identity evidence changed/),late=await nextRequest(h,'conversation/checkpointValueRead',2);
 const ending=h.session.detach(),detach=await nextRequest(h,'session/detach');h.transport.respond(detach.id,{type:'detached'});await ending;
 h.transport.respond(late.id,{type:'checkpoint_value_chunk',chunk:chunk(0,bytes.length)});await rejected;h.client.close();
});


it('T14 T17 Child event revision must match its own record before fold or Applied',()=>{
 const state=snapshot(),baseline=checkpoint(state,'4');baseline.read_cut.activation_id='actual-a';
 const identity={agent_id:'child',conversation_id:state.conversation_id,activation_id:'actual-a'};
 const child=new ChildObservation(identity.agent_id,identity.conversation_id,identity.activation_id);
 child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'4'},checkpoint:baseline});
 const before=child.snapshot;
 assert.throws(()=>child.apply({type:'events',after:{...identity,cursor:'4'},cursor:{...identity,cursor:'5'},events:[{cursor:'5',event:{type:'projection_transition',events:[{type:'runtime_shutdown'}],metadata:{read_cut:baseline.read_cut,collections:[],content:[]}}}]}),/Child event cut evidence mismatch/);
 assert.equal(child.snapshot,before);assert.equal(child.snapshot?.shutting_down,false);assert.equal(child.checkpoint,baseline);assert.equal(child.appliedCursor?.cursor,'4');assert.equal(child.phase,'resyncRequired');
 const repaired=checkpoint(state,'5');repaired.read_cut.activation_id=identity.activation_id;
 child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'5'},checkpoint:repaired});
 child.apply({type:'events',after:{...identity,cursor:'5'},cursor:{...identity,cursor:'7'},events:['6','7'].map(cursor=>({cursor,event:{type:'projection_transition' as const,events:[],metadata:{read_cut:{...repaired.read_cut,revision:cursor},collections:[],content:[]}}}))});
 assert.equal(child.appliedCursor?.cursor,'7');assert.equal(child.checkpoint?.read_cut.revision,'7');
});

it('typed unavailable fences only the exact registration without consuming its next cursor',()=>{
 const chain=new ObservationChain<string>();chain.installCheckpoint('7');chain.beginSubscription('7');chain.acceptReceipt('original','7');let folds=0;
 const fold=()=>{folds++;};
 assert.equal(chain.deliver({registrationId:'foreign',unavailable:true},fold),'ignored');
 assert.equal(chain.deliver({registrationId:'original',unavailable:true},fold),'unavailable');
 assert.equal(chain.applied,'7');assert.equal(chain.phase,'unavailable');
 assert.equal(chain.deliver({registrationId:'original',cursor:'8',event:'old'},fold),'ignored');assert.equal(folds,0);
 chain.installCheckpoint('7');chain.beginSubscription('7');chain.acceptReceipt('recovered','7');
 assert.equal(chain.deliver({registrationId:'original',unavailable:true},fold),'ignored');
 assert.equal(chain.deliver({registrationId:'recovered',cursor:'8',event:'actual'},fold),'applied');assert.equal(folds,1);assert.equal(chain.applied,'8');
});
