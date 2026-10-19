import {act,cleanup,fireEvent,render,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointReadCut,CheckpointValueChunk,TraceFieldSource} from '../../protocol/app-server/v51';
import {TraceFields} from '../src/app/trajectory/TraceFields';
import type {TraceFieldReader} from '../src/client/trace';

afterEach(cleanup);
const cut=(revision:string):CheckpointReadCut=>({conversation_id:'original',revision,through:revision,domain_revisions:{},collection_cuts:{}});
const field:Extract<TraceFieldSource,{type:'value'}>={type:'value',field:'model',domain:'trace.records.9.detail.request.model',item_offset:0,path:[],encoded_bytes:'5'};
const chunk=(scope:CheckpointReadCut):CheckpointValueChunk=>({cut:scope,domain:field.domain,item_offset:0,path:[],offset:0,bytes_base64:btoa('model'),readable_end:5,next_offset:null,representation:'utf8_text'});
const ports=(value:TraceFieldReader['value']):TraceFieldReader=>({value,content:async()=>{throw Error('must not read whole content');},page:async()=>{throw Error('must not read whole collection');}});

it('an explicitly selected original field uses the standalone page cut rather than an older checkpoint',async()=>{
 const read=vi.fn(async(scope:CheckpointReadCut)=>chunk(scope)),reader=ports(read),scope=cut('9');
 const ui=render(<TraceFields fields={[field]} cut={scope} reader={reader} owned={()=>true}/>);
 expect(read).not.toHaveBeenCalled();fireEvent.click(ui.getByText('Read original value'));
 await waitFor(()=>expect(ui.getByText('model')).toBeTruthy());
 expect(read.mock.calls[0]?.[0]).toEqual(scope);expect(read.mock.calls[0]?.[0]).not.toEqual(cut('1'));
});
it('changing the selected native cut retires a pending field window even when its source selector is unchanged',async()=>{
 let release!:(chunk:CheckpointValueChunk)=>void;
 const read=vi.fn(()=>new Promise<CheckpointValueChunk>(resolve=>{release=resolve;})),reader=ports(read),old=cut('4');
 const ui=render(<TraceFields fields={[field]} cut={old} reader={reader} owned={()=>true}/>);
 fireEvent.click(ui.getByText('Read original value'));ui.rerender(<TraceFields fields={[field]} cut={cut('5')} reader={reader} owned={()=>true}/>);
 await act(async()=>release(chunk(old)));expect(ui.container.querySelector('pre')).toBeNull();expect(read).toHaveBeenCalledTimes(1);
});
it('missing source cuts keep facts visible but cannot invoke a reader',()=>{
 const read=vi.fn(async()=>chunk(cut('4'))),reader=ports(read);
 const ui=render(<TraceFields fields={[field]} reader={reader} owned={()=>true}/>);
 expect(ui.getByText('Original Trace field cut is unavailable.')).toBeTruthy();expect((ui.getByText('Read original value') as HTMLButtonElement).disabled).toBe(true);expect(read).not.toHaveBeenCalled();
});
it.each(['ownership','reader'] as const)('a held original Value reply cannot outlive its current %s',async replacement=>{
 let release!:(chunk:CheckpointValueChunk)=>void,owns!:()=>boolean;const scope=cut('9');
 const read=vi.fn((_cut:CheckpointReadCut,_field:unknown,_offset:number,current:()=>boolean)=>{owns=current;return new Promise<CheckpointValueChunk>(resolve=>{release=resolve;});}),reader=ports(read);
 const ui=render(<TraceFields fields={[field]} cut={scope} reader={reader} owned={()=>true}/>);fireEvent.click(ui.getByText('Read original value'));expect(owns()).toBe(true);
 ui.rerender(<TraceFields fields={[field]} cut={scope} reader={replacement==='reader'?ports(read):reader} owned={()=>replacement==='reader'}/>);expect(owns()).toBe(false);await act(async()=>release(chunk(scope)));expect(ui.container.querySelector('pre')).toBeNull();expect(ui.queryByRole('alert')).toBeNull();expect(read).toHaveBeenCalledTimes(1);
});
it('a held Value read and continuation survive new same-owner callbacks at the unchanged cut',async()=>{
 let release!:(chunk:CheckpointValueChunk)=>void,owns!:()=>boolean;const scope=cut('9'),original={...field,encoded_bytes:'10'};
 const read=vi.fn((at:CheckpointReadCut,_field:unknown,offset:number,current:()=>boolean)=>{owns=current;return offset?Promise.resolve({...chunk(at),offset:5,bytes_base64:btoa('again'),readable_end:10}):new Promise<CheckpointValueChunk>(resolve=>{release=resolve;});}),reader=ports(read);
 const ui=render(<TraceFields fields={[original]} cut={scope} reader={reader} owned={()=>true}/>);fireEvent.click(ui.getByText('Read original value'));ui.rerender(<TraceFields fields={[original]} cut={scope} reader={reader} owned={()=>true}/>);expect(owns()).toBe(true);
 await act(async()=>release({...chunk(scope),readable_end:10,next_offset:5}));expect(ui.container.querySelector('pre')?.textContent).toBe('model');ui.rerender(<TraceFields fields={[original]} cut={scope} reader={reader} owned={()=>true}/>);fireEvent.click(ui.getByText('Read next chunk'));await waitFor(()=>expect(ui.container.querySelector('pre')?.textContent).toBe('again'));expect(ui.container.querySelectorAll('pre')).toHaveLength(1);expect(read.mock.calls.map(args=>args[2])).toEqual([0,5]);expect(read.mock.calls.every(args=>args[0]===scope)).toBe(true);
});
