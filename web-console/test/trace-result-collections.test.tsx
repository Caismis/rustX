import {act,cleanup,render,screen,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,ContentChunk,ContentRef,TraceInspectionHead,TraceRecord} from '../../protocol/app-server/v57';
import {TrajectoryInspector,type TrajectoryInspectorProps} from '../src/app/trajectory/TrajectoryInspector';
import {bindTraceDetailReader} from '../src/client/trace';
import {traceTool} from './trace-fixture';
import {checkpoint,snapshot} from './fixture';
afterEach(cleanup);
const cut=()=>({...checkpoint(snapshot()).read_cut,revision:'9',through:'300'});
function fixture(){
 const original=cut(),record=traceTool(295),resultDomain='messages.ledger:3.blocks',metadata='{"tool_call_id":"chat-image","result":{"artifacts":[],"status":{"type":"success"}}}',image='{ "type":"image", "artifact_id":"artifact_1" }';
 const ref:ContentRef={conversation_id:original.conversation_id,source:{type:'canonical_message_block',component:'message_metadata',locator:{type:'by_ledger_position',position:3},block_index:0},representation:'canonical_json_v1',version:1,content_revision:1,readable_end:new TextEncoder().encode(metadata).length};
 record.fields=[{type:'content',field:'result',binding:{path:[],source_path:['result'],content_ref:ref,deferred:true}}];record.collections=[{domain:resultDomain,revision:'1',total:'1',loaded:'0',next_offset:'0'}];
 const head:TraceInspectionHead={id:record.id,kind:'tool',record,content:{type:'tool',definition:{type:'unavailable'}}},page:CheckpointPage={read_cut:original,cursor:original.revision,domain:'trace.records.295.detail',total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:record.id,key:null,value:JSON.parse(JSON.stringify(head)),content:[],deferred:true,body_complete:false}]};
 const imageRef:ContentRef={...ref,source:{type:'canonical_message_block',component:'block_metadata',locator:{type:'by_ledger_position',position:3},block_index:0},readable_end:new TextEncoder().encode(image).length};
 const value=vi.fn(async()=>{throw Error('Canonical whole ValueRead is forbidden.');}),content=vi.fn(async(_cut:unknown,reference:ContentRef,offset:number)=>{const raw=reference.source.type==='canonical_message_block'&&reference.source.component==='block_metadata'?image:metadata;return chunk(reference,raw,offset);}),readPage=vi.fn(async():Promise<CheckpointPage>=>({read_cut:original,cursor:original.revision,domain:resultDomain,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'image',key:null,value:null,content:[{path:['value'],source_path:null,content_ref:imageRef,deferred:false}],deferred:false,body_complete:null}]} satisfies CheckpointPage));
 let active=true;const reader=bindTraceDetailReader({value,content,page:readPage},page,()=>active);
 return {record,original,page,reader,value,content,readPage,metadata,image,ref,imageRef,retire:()=>{active=false;}};
}
function chunk(reference:ContentRef,raw:string,offset=0):ContentChunk{const bytes=new TextEncoder().encode(raw),end=Math.min(offset+65536,bytes.length);return {content_ref:reference,offset,data_base64:btoa(String.fromCharCode(...bytes.slice(offset,end))),end_offset:end,readable_end:bytes.length,next_offset:end<bytes.length?end:null,complete:end===bytes.length};}
const props=(record:TraceRecord):TrajectoryInspectorProps=>({item:{type:'RecordRow',record,display_key:'tool',owner_record_id:record.id,facet:'output',label:'Tool',preview:''},facet:'output',onFacet:vi.fn(),onLoadDetail:vi.fn(),onClose:vi.fn(),records:[record],executions:new Map(),requestNumbers:new Map(),completeHistory:true,onOpen:vi.fn()});
function only(f:ReturnType<typeof fixture>,mode:'field'|'collection') {
 const head=f.page.items[0].value as unknown as TraceInspectionHead;
 if(mode==='field')head.record.collections=[];else {head.record.fields=[];if(head.record.tool)head.record.tool.fields=[];}
}
it('opening Tool Result automatically renders original metadata and result blocks',async()=>{
 const f=fixture();render(<TrajectoryInspector {...props(f.record)} detailPage={f.page} detailReader={f.reader}/>);
 await screen.findByText('artifact_1');expect(screen.queryByText(f.metadata)).toBeNull();expect(screen.queryByText(f.image)).toBeNull();
 expect(f.readPage.mock.calls[0].slice(0,3)).toEqual([f.original,f.record.collections[0].domain,0]);
 expect(f.content.mock.calls.map(args=>args.slice(0,3))).toEqual([[f.original,f.imageRef,0],[f.original,f.ref,0]]);expect(f.value).not.toHaveBeenCalled();
 expect(screen.queryByRole('combobox',{name:'Collection'})).toBeNull();expect(screen.queryByRole('button',{name:/Read content|Read first page/})).toBeNull();
});
it.each(['field','collection'] as const)('same-owner rerenders preserve held %s reads and assemble continuation bytes',async mode=>{
 const f=fixture(),raw=mode==='field'?' {"result":{"status":{"type":"success"},"original":"'+'x'.repeat(66000)+'","opaque":18446744073709551615}} ':' {"original":"'+'x'.repeat(66000)+'","opaque":18446744073709551615} ',bytes=new TextEncoder().encode(raw);let finish!:(value:ContentChunk)=>void;
 only(f,mode);const head=f.page.items[0].value as unknown as TraceInspectionHead;
 if(mode==='field'){const field=head.record.fields[0];if(field.type!=='content')throw Error('fixture');field.binding.content_ref.readable_end=bytes.length;}
 else {f.imageRef.readable_end=bytes.length;f.imageRef.source={...f.imageRef.source,component:'payload'};f.readPage.mockResolvedValue({read_cut:f.original,cursor:f.original.revision,domain:f.record.collections[0].domain,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:'json',key:null,value:null,content:[{path:['value'],source_path:null,content_ref:f.imageRef,deferred:true}],deferred:true,body_complete:null}]});}
 f.content.mockImplementation(async(_cut,reference,offset)=>offset?chunk(reference,raw,offset):new Promise(resolve=>{finish=resolve;}));
 const ui=render(<TrajectoryInspector {...props(f.record)} detailPage={f.page} detailReader={f.reader}/>);
 await waitFor(()=>expect(f.content).toHaveBeenCalledTimes(1));const reference=f.content.mock.calls[0][1];
 ui.rerender(<TrajectoryInspector {...props({...f.record,state:'running'})} records={[f.record,traceTool(999)]} detailPage={f.page} detailReader={f.reader}/>);
 await act(async()=>finish(chunk(reference,raw)));if(mode==='collection')await screen.findByText('18446744073709551615');else await screen.findByText('No output');expect(document.body.textContent).not.toContain('18446744073709552000');
 ui.rerender(<TrajectoryInspector {...props({...f.record,state:'completed'})} records={[traceTool(998),f.record]} detailPage={f.page} detailReader={f.reader}/>);
 expect(f.content.mock.calls.map(args=>args[2])).toEqual([0,65536]);expect(f.value).not.toHaveBeenCalled();
});
it.each([['reader','field'],['cut','field'],['reader','collection'],['cut','collection']] as const)('replacing %s rejects a held original %s reply',async(replacement,mode)=>{
 const f=fixture();only(f,mode);let finish!:(value:ContentChunk)=>void;f.content.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
 const ui=render(<TrajectoryInspector {...props(f.record)} detailPage={f.page} detailReader={f.reader}/>);
 await waitFor(()=>expect(f.content).toHaveBeenCalledTimes(1));const originalFinish=finish,reference=f.content.mock.calls[0][1],text=mode==='field'?f.metadata:f.image;
 const next=structuredClone(f.page);if(replacement==='cut'){next.cursor='10';next.read_cut.revision='10';}
 const nextReader={...f.reader,content:vi.fn(()=>new Promise<ContentChunk>(()=>{}))};
 ui.rerender(<TrajectoryInspector {...props(f.record)} detailPage={next} detailReader={replacement==='reader'?nextReader:f.reader}/>);
 await act(async()=>originalFinish(chunk(reference,text)));expect(screen.queryByText(text)).toBeNull();
});
it.each(['initial','replacement'] as const)('result blocks remain reachable without metadata fields (%s)',async mode=>{
 const f=fixture(),without=structuredClone(f.page),head=without.items[0].value as unknown as TraceInspectionHead;head.record.fields=[];if(head.record.tool)head.record.tool.fields=[];
 const ui=render(<TrajectoryInspector {...props(f.record)} detailPage={mode==='initial'?without:f.page} detailReader={f.reader}/>);
 if(mode==='replacement')ui.rerender(<TrajectoryInspector {...props(f.record)} detailPage={without} detailReader={f.reader}/>);
 await screen.findByText('artifact_1');expect(f.readPage.mock.calls[0].slice(0,3)).toEqual([f.original,f.record.collections[0].domain,0]);
 expect(f.content.mock.calls.some(args=>JSON.stringify(args[1])===JSON.stringify(f.imageRef))).toBe(true);expect(f.value).not.toHaveBeenCalled();
});
