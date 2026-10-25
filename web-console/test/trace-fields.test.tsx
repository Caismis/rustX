import {act,cleanup,render,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointReadCut,CheckpointValueChunk,TraceFieldSource} from '../../protocol/app-server/v57';
import {TraceFields} from '../src/app/trajectory/TraceFields';
import type {TraceFieldReader} from '../src/client/trace';
afterEach(cleanup);
const cut=(revision:string):CheckpointReadCut=>({conversation_id:'original',revision,through:revision,domain_revisions:{},collection_cuts:{}});
const field:Extract<TraceFieldSource,{type:'value'}>={type:'value',field:'model',domain:'trace.records.9.detail.request.model',item_offset:0,path:[],encoded_bytes:'5'};
const chunk=(scope:CheckpointReadCut):CheckpointValueChunk=>({cut:scope,domain:field.domain,item_offset:0,path:[],offset:0,bytes_base64:btoa('model'),readable_end:5,next_offset:null,representation:'utf8_text'});
const ports=(value:TraceFieldReader['value']):TraceFieldReader=>({value,content:async()=>{throw Error('must not read whole content');},page:async()=>{throw Error('must not read whole collection');}});
it('opening a selected field automatically reads its own retained cut',async()=>{
 const read=vi.fn(async(scope:CheckpointReadCut)=>chunk(scope)),reader=ports(read),scope=cut('9');
 const ui=render(<TraceFields fields={[field]} cut={scope} reader={reader} owned={()=>true}/>);
 await ui.findByText('model');expect(read.mock.calls[0]?.[0]).toEqual(scope);expect(ui.queryAllByRole('button')).toHaveLength(0);
});
it('changing the native cut rejects a pending reply while reading the replacement',async()=>{
 const pending=new Map<string,(value:CheckpointValueChunk)=>void>();
 const read=vi.fn((scope:CheckpointReadCut)=>new Promise<CheckpointValueChunk>(resolve=>{pending.set(scope.revision,resolve);})),reader=ports(read),old=cut('4');
 const ui=render(<TraceFields fields={[field]} cut={old} reader={reader} owned={()=>true}/>);ui.rerender(<TraceFields fields={[field]} cut={cut('5')} reader={reader} owned={()=>true}/>);
 await act(async()=>pending.get('4')!(chunk(old)));expect(ui.container.querySelector('pre')).toBeNull();expect(read).toHaveBeenCalledTimes(2);
});
it('missing source cuts do not invoke a reader',()=>{
 const read=vi.fn(async()=>chunk(cut('4'))),ui=render(<TraceFields fields={[field]} reader={ports(read)} owned={()=>true}/>);
 expect(ui.getByRole('status')).toBeTruthy();expect(ui.queryAllByRole('button')).toHaveLength(0);expect(read).not.toHaveBeenCalled();
});
it.each(['ownership','reader'] as const)('retiring a selected %s rejects its held reply',async replacement=>{
 let finish!:(value:CheckpointValueChunk)=>void,owns!:()=>boolean;
 const read=vi.fn((_cut:CheckpointReadCut,_field:unknown,_offset:number,current:()=>boolean)=>{owns=current;return new Promise<CheckpointValueChunk>(resolve=>{finish=resolve;});}),reader=ports(read),scope=cut('9');
 const ui=render(<TraceFields fields={[field]} cut={scope} reader={reader} owned={()=>true}/>);const originalOwns=owns,originalFinish=finish;
 ui.rerender(<TraceFields fields={[field]} cut={scope} reader={replacement==='reader'?ports(read):reader} owned={()=>replacement==='reader'}/>);expect(originalOwns()).toBe(false);
 await act(async()=>originalFinish(chunk(scope)));expect(ui.container.querySelector('pre')).toBeNull();expect(ui.queryByRole('alert')).toBeNull();
});
it('same-owner callbacks retain the read and all continuation bytes are assembled automatically',async()=>{
 let finish!:(value:CheckpointValueChunk)=>void,owns!:()=>boolean;const scope=cut('9'),original={...field,encoded_bytes:'10'};
 const read=vi.fn((at:CheckpointReadCut,_field:unknown,offset:number,current:()=>boolean)=>{owns=current;return offset?Promise.resolve({...chunk(at),offset:5,bytes_base64:btoa('again'),readable_end:10}):new Promise<CheckpointValueChunk>(resolve=>{finish=resolve;});}),reader=ports(read);
 const ui=render(<TraceFields fields={[original]} cut={scope} reader={reader} owned={()=>true}/>);ui.rerender(<TraceFields fields={[original]} cut={scope} reader={reader} owned={()=>true}/>);expect(owns()).toBe(true);
 await act(async()=>finish({...chunk(scope),readable_end:10,next_offset:5}));await waitFor(()=>expect(ui.container.querySelector('pre')?.textContent).toBe('modelagain'));
 expect(read.mock.calls.map(call=>call[2])).toEqual([0,5]);expect(read.mock.calls.every(call=>call[0]===scope)).toBe(true);
});
