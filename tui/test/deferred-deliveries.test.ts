/** Client correlation and presentation ownership; actual native socket authority is proved in Rust. */
import assert from 'node:assert/strict';
import {it} from 'node:test';
import {harness} from './support/app-server-harness.ts';
import {snapshot} from './support/fixtures.ts';
import type {RuntimeClientDeliveryHead,RuntimeClientDeliveryItem,SessionFileReference} from '../../protocol/app-server/v57.ts';

it('finite delivery discovery preserves the source cut, ninth original index, selected metadata and protected RPC ownership',async()=>{
 const original:SessionFileReference={scope:{conversation_id:'conv_01900000-0000-7000-8000-000000000004',device:'1',inode:'2'},path:'out/报告.md',name:'报告.md',mime_type:'text/markdown'};
 const bytes=Buffer.from(JSON.stringify(original)),locator={type:'by_ledger_position' as const,position:2};
 const item=(index:number):RuntimeClientDeliveryItem=>({index,locator,file:null,metadata_deferred:true,encoded_bytes:String(bytes.length),metadata_domain:`messages.ledger:2.deliveries.${index}.metadata`});
 const head:RuntimeClientDeliveryHead={domain:'messages.ledger:2.deliveries',total:'9',items:Array.from({length:8},(_,index)=>({...item(index),file:original,metadata_deferred:false})) as RuntimeClientDeliveryHead["items"],next_offset:8};
 const initial={...snapshot(),message_descriptors:[{locator,role:'tool' as const,kind:'message',block_count:'1',tool_call_count:'0',tool_calls:[],turn_process:null,completed_response:null,deliveries:head,content:[],collections:[]}]};
 const h=await harness(initial),cut=h.session.checkpoint!.read_cut;
 try {
  const pending=h.session.deliveryPage();
  const first=(await h.transport.log.awaitMethod('conversation/checkpointPage',1))[0]!;
  assert.equal(first.method,'conversation/checkpointPage');if(first.method!=='conversation/checkpointPage')throw new Error('method');
  assert.deepEqual(first.params,{target:h.target,agent_id:null,cut,domain:'transcript.entries',offset:0,limit:32});
  h.transport.respond(first.id,{type:'checkpoint_page',page:{cursor:'0',read_cut:cut,domain:'transcript.entries',total:'0',items:[],next_offset:null,collections:[]}});
  const page=await pending;assert.deepEqual(page.records[0]?.file,original);assert.equal(page.records[0]?.conversationId,h.target.conversation_id);
  // The collection continuation is native source evidence; exercise its actual ninth ordinal.
  const collection=page.collections![0]!;
  const next=h.session.deliveryCollection(collection),request=(await h.transport.log.awaitMethod('conversation/checkpointPage',2))[1]!;
  if(request.method!=='conversation/checkpointPage')throw new Error('method');assert.deepEqual(request.params,{target:h.target,agent_id:null,cut,domain:head.domain,offset:8,limit:8});
  h.transport.respond(request.id,{type:'checkpoint_page',page:{cursor:'0',read_cut:cut,domain:head.domain,total:'9',items:[{offset:'8',identity:'delivery 8',value:item(8),content:[],deferred:true}],next_offset:null,collections:[]}});
  const record=(await next).records[0]!;assert.equal(record.file,undefined);assert.equal(record.count,'9');
  const reading=h.session.deliveryMetadata(record,()=>true);
  for(let offset=0,number=1;offset<bytes.length;number++) {
   const selected=(await h.transport.log.awaitMethod('conversation/checkpointValueRead',number))[number-1]!;
   if(selected.method!=='conversation/checkpointValueRead')throw new Error('method');
   assert.deepEqual(selected.params,{target:h.target,agent_id:null,cut,domain:item(8).metadata_domain,item_offset:0,path:[],offset,max_bytes:65536});
   const end=Math.min(bytes.length,offset+11);
   h.transport.respond(selected.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut,domain:item(8).metadata_domain,item_offset:0,path:[],offset,bytes_base64:bytes.subarray(offset,end).toString('base64'),readable_end:bytes.length,next_offset:end===bytes.length?null:end}});offset=end;
  }
  assert.deepEqual(await reading,original);
  const protectedRead=h.session.readDelivery(record),read=(await h.transport.log.awaitMethod('delivery/read'))[0]!;
  if(read.method!=='delivery/read')throw new Error('method');assert.deepEqual(read.params,{target:h.target,locator,delivery_index:8});
  h.transport.respond(read.id,{type:'session_file_bytes',file:original,data:Buffer.from('original bytes').toString('base64')});
  assert.deepEqual(await protectedRead,{file:original,data:Buffer.from('original bytes').toString('base64')});
  await assert.rejects(h.session.readDelivery({...record,conversationId:'foreign'}),/locator source changed/);assert.equal(h.transport.log.count('delivery/read'),1);
  let current=true;const number=h.transport.log.count('conversation/checkpointValueRead')+1;const late=h.session.deliveryMetadata(record,()=>current),rejected=assert.rejects(late,/view changed/);
  const selected=(await h.transport.log.awaitMethod('conversation/checkpointValueRead',number))[number-1]!;current=false;
  h.transport.respond(selected.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut,domain:item(8).metadata_domain,item_offset:0,path:[],offset:0,bytes_base64:bytes.toString('base64'),readable_end:bytes.length,next_offset:null}});await rejected;
  assert.equal(h.session.state.cursor,'0');assert.equal(h.transport.log.count('turn/start'),0);assert.equal(h.transport.log.count('conversation/contentRead'),0);
 }finally{h.client.close();}
});
