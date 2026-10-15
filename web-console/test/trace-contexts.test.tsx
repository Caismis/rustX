import {act,cleanup,fireEvent,render,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,CheckpointReadCut,TraceContextPresentation} from '../../protocol/app-server/v48';
import {TraceContexts} from '../src/app/trajectory/TraceContexts';
import type {TraceFieldReader} from '../src/client/trace';
import {traceRecord} from './trace-fixture';
afterEach(cleanup);
const cut:CheckpointReadCut={conversation_id:'original',revision:'4',through:'4',domain_revisions:{},collection_cuts:{}};
const row=(index:number):TraceContextPresentation=>({locator:{type:'by_ledger_position',position:index+1},message_id:null,fields:[],producer:{Native:'workspace_instructions'},context_kind:'native_environment',source:{type:'runtime'},preview:{text:`original context ${index}`,truncated:false},attachments:[],truncated:false});
const request=()=>({...traceRecord(1).request!,context_domain:'trace.records.1.context',context_total:'3',context_additions:[row(0)],context_next_offset:1});
const page=():CheckpointPage=>({cursor:'4',read_cut:cut,domain:'trace.records.1.context',total:'3',items:[1,2].map(index=>({offset:String(index),identity:`context ${index}`,value:row(index),content:[],deferred:false})),next_offset:null});
const reader=(read:TraceFieldReader['page']):TraceFieldReader=>({page:read,value:async()=>{throw Error('must not hydrate whole Context');},content:async()=>{throw Error('must not hydrate whole Context');}});
it('native Context continuation uses original returned cut and immutable offsets without collecting whole history',async()=>{
 const read=vi.fn(async()=>page()),port=reader(read),ui=render(<TraceContexts request={request()} cut={cut} reader={port} owned={()=>true}/>);
 expect(read).not.toHaveBeenCalled();fireEvent.click(ui.getByText('Read original value'));
 await waitFor(()=>expect(ui.getByText('original context 1')).toBeTruthy());expect(ui.getByText('original context 2')).toBeTruthy();
 expect(read.mock.calls[0]).toEqual([cut,'trace.records.1.context',1,expect.any(Function)]);
});
it('late Context page cannot repaint a new selected cut',async()=>{
 let release!:(page:CheckpointPage)=>void;const read=vi.fn(()=>new Promise<CheckpointPage>(resolve=>{release=resolve;})),port=reader(read);
 const ui=render(<TraceContexts request={request()} cut={cut} reader={port} owned={()=>true}/>);fireEvent.click(ui.getByText('Read original value'));
 ui.rerender(<TraceContexts request={request()} cut={{...cut,revision:'5'}} reader={port} owned={()=>true}/>);
 await act(async()=>release(page()));expect(ui.queryByText('original context 1')).toBeNull();expect(read).toHaveBeenCalledTimes(1);
});
it('oversized Context pages are refused before visiting their selected values',async()=>{
 let visits=0;const oversized={...page(),items:Array.from({length:33},()=>({offset:'1',identity:'untrusted',get value(){visits++;throw Error('must not visit');},content:[],deferred:false}))};
 const ui=render(<TraceContexts request={request()} cut={cut} reader={reader(async()=>oversized)} owned={()=>true}/>);fireEvent.click(ui.getByText('Read original value'));
 await waitFor(()=>expect(ui.getByRole('alert').textContent).toContain('Original Context page changed'));expect(visits).toBe(0);expect(ui.getByText('3')).toBeTruthy();
});
