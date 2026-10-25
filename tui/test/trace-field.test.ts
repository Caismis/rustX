import assert from 'node:assert/strict';
import {it} from 'node:test';
import type {CheckpointReadCut,CheckpointValueChunk} from '../../protocol/app-server/v57.ts';
import {readTraceFieldWindow,type TraceValueField} from '../../protocol/app-server/trace-field.ts';

const cut:CheckpointReadCut={conversation_id:'original',revision:'4',through:'4',domain_revisions:{},collection_cuts:{}};
const source=(bytes:number):TraceValueField=>({type:'value',field:'model',domain:'trace.records.4.detail.request.model',item_offset:0,path:[],encoded_bytes:String(bytes)});
const chunk=(field:TraceValueField,bytes:Uint8Array,offset=0,next:number|null=null):CheckpointValueChunk=>({cut,domain:field.domain,item_offset:0,path:[],representation:'utf8_text',offset,bytes_base64:Buffer.from(bytes).toString('base64'),readable_end:Number(field.encoded_bytes),next_offset:next});

it('reads exact original BOM and split Unicode windows without whole-value hydration',async()=>{
 const bytes=Buffer.from('\uFEFF original 模型'),field=source(bytes.length),decoder=new TextDecoder('utf-8',{ignoreBOM:true,fatal:true});
 let text='';
 for(let offset=0;offset<bytes.length;){const end=Math.min(bytes.length,offset+2);const range=await readTraceFieldWindow(field,cut,offset,async()=>chunk(field,bytes.subarray(offset,end),offset,end<bytes.length?end:null),()=>true);assert.ok(range.bytes.length<=2);text+=decoder.decode(range.bytes,{stream:range.next!=null});offset=end;}
 assert.equal(text,'\uFEFF original 模型');
});
it('original Canonical JSON wide integer bytes are never parsed or rounded',async()=>{
 const bytes=Buffer.from('{"opaque":18446744073709551615}'),field=source(bytes.length);
 const range=await readTraceFieldWindow(field,cut,0,async()=>({...chunk(field,bytes),representation:'canonical_json_v1'}),()=>true);
 assert.equal(Buffer.from(range.bytes).toString(),'{"opaque":18446744073709551615}');
});
it('retains the selected cut and refuses foreign or late replies without retry',async()=>{
 const field=source(1);
 await assert.rejects(readTraceFieldWindow(field,cut,0,async()=>({...chunk(field,Buffer.from('a')),cut:{...cut,revision:'5'}}),()=>true),/identity changed/);
 let current=true,reads=0;
 await assert.rejects(readTraceFieldWindow(field,cut,0,async()=>{reads++;current=false;return chunk(field,Buffer.from('a'));},()=>current),/view was replaced/);
 assert.equal(reads,1);
});
it('checks original extent and encoded chunk bounds before allocating decoded bytes',async()=>{
 let reads=0;
 await assert.rejects(readTraceFieldWindow({...source(1),encoded_bytes:'18446744073709551615'},cut,0,async()=>{reads++;throw Error('must not read');},()=>true),/coordinates are unavailable/);
 assert.equal(reads,0);
 const field=source(65537);
 await assert.rejects(readTraceFieldWindow(field,cut,0,async()=>chunk(field,new Uint8Array(65537)),()=>true),/decoded-byte bound/);
 await assert.rejects(readTraceFieldWindow(source(1),cut,0,async()=>chunk(source(1),new Uint8Array(),0,0),()=>true),/identity changed/);
});
