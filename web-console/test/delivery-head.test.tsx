import {act,cleanup,fireEvent,render,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import {TurnDeliveries} from '../src/app/agent/TurnDeliveries';
import {JournalSourceProvider} from '../src/app/agent/ObservationSource';
import {PreviewContext} from '../src/app/components/ArtifactPreview';
import {checkpoint,Server,snapshot} from './fixture';
import type {CheckpointPage,MethodResult,RuntimeClientDeliveryHead,RuntimeClientDeliveryItem,SessionFileReference} from '../../protocol/app-server/v60';
import {deliveryPage,readDeliveryMetadata} from '../../protocol/app-server/delivery-observation';
let server:Server;
afterEach(()=>{cleanup();server?.client.disconnect();});
const file:SessionFileReference={scope:{conversation_id:'original-inherited-owner',device:'1',inode:'2'},path:'out/report.md',name:'report.md',mime_type:'text/markdown',description:'native selected metadata'};
const locator={type:'by_ledger_position' as const,position:2};
const item=(index:number):RuntimeClientDeliveryItem=>({index,locator,file:null,metadata_deferred:true,encoded_bytes:String(new TextEncoder().encode(JSON.stringify(file)).length),metadata_domain:`messages.ledger:2.deliveries.${index}.metadata`});
const head:RuntimeClientDeliveryHead={domain:'messages.ledger:2.deliveries',total:'9',items:Array.from({length:8},(_,index)=>({...item(index),file:{...file,path:`out/report-${index}.md`},metadata_deferred:false})) as RuntimeClientDeliveryHead['items'],next_offset:8};
it.each([null,'tool-result'])('successful C ninth-item discovery preserves native read ownership and preview identity %s',async(messageIdentity)=>{
 server=new Server();await server.attached('A');const cut=checkpoint(snapshot()).read_cut,target=server.target('A'),openPreview=vi.fn(),download=vi.fn();
 const previewLocator=messageIdentity==null?locator:{type:'by_message_id',message_id:messageIdentity};
 const selected=item(8),original=new TextEncoder().encode(JSON.stringify(file));
 server.handlers.set('conversation/checkpointPage',request=>{
  if(request.method!=='conversation/checkpointPage')throw new Error('method');
  expect(request.params).toEqual({target,agent_id:null,cut,domain:head.domain,offset:8,limit:8});
  return {type:'checkpoint_page',page:{cursor:'0',read_cut:cut,domain:head.domain,total:'9',items:[{offset:'8',identity:'delivery 8',value:selected,content:[],deferred:false}],next_offset:null,collections:[]}};
 });
 server.handlers.set('conversation/checkpointValueRead',request=>{
  if(request.method!=='conversation/checkpointValueRead')throw new Error('method');
  expect(request.params).toMatchObject({target,agent_id:null,cut,domain:selected.metadata_domain,item_offset:0,path:[],max_bytes:65536});
  const offset=request.params.offset,end=Math.min(original.length,offset+17);
  return {type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut,domain:selected.metadata_domain,item_offset:0,path:[],offset,bytes_base64:btoa(String.fromCharCode(...original.slice(offset,end))),readable_end:original.length,next_offset:end===original.length?null:end}};
 });
 const source={client:server.client,target,cut:()=>cut,owned:()=>true};
 const ui=render(<JournalSourceProvider source={source}><PreviewContext.Provider value={{openPreview,download}}><TurnDeliveries conversationId="conversation-A" batches={[{position:'2',messageIdentity,head}]}/></PreviewContext.Provider></JournalSourceProvider>);
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Show more'})));
 expect(ui.queryByRole('button',{name:'Show more'})).toBeNull();
 await waitFor(()=>expect(ui.getByRole('button',{name:'Show all 9 delivered files'})).toBeTruthy());
 fireEvent.click(ui.getByRole('button',{name:'Show all 9 delivered files'}));
 expect(ui.getAllByRole('button',{name:'Preview report.md in sidebar'})).toHaveLength(9);
 expect(ui.queryByRole('button',{name:'Preview 9/9'})).toBeNull();
 fireEvent.click(ui.getAllByRole('button',{name:'Preview report.md in sidebar'}).at(-1)!);
 expect(openPreview).toHaveBeenCalledWith(expect.objectContaining({source:expect.objectContaining({conversationId:target.conversation_id,locator:previewLocator,index:8,file}),name:file.name}));
 fireEvent.click(ui.getAllByRole('button',{name:'Download report.md'}).at(-1)!);
 expect(download).toHaveBeenCalledWith(expect.objectContaining({source:expect.objectContaining({locator:previewLocator,index:8,file})}));
 expect(server.requests.some(row=>['conversation/contentRead','session/modelGet'].includes(row.request.method))).toBe(false);
 expect(server.client.getSnapshot().views.A.cursor).toBe('0');
});
it('a metadata response after exact owner retirement cannot dispatch Preview or Download',async()=>{
 server=new Server();await server.attached('A');const cut=checkpoint(snapshot()).read_cut,target=server.target('A'),openPreview=vi.fn(),download=vi.fn(),selected=item(0);let owned=true;
 server.held.add('conversation/checkpointValueRead');
 const ui=render(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>owned}}><PreviewContext.Provider value={{openPreview,download}}><TurnDeliveries conversationId="conversation-A" batches={[{position:'2',head:{...head,items:[selected],next_offset:null}}]}/></PreviewContext.Provider></JournalSourceProvider>);
 expect(ui.queryByRole('button',{name:'Preview 1/9'})).toBeNull();
 const request=await server.waitFor('conversation/checkpointValueRead',1);owned=false;
 await act(async()=>server.socket.success(request,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut,domain:selected.metadata_domain,item_offset:0,path:[],offset:0,bytes_base64:btoa(JSON.stringify(file)),readable_end:Number(selected.encoded_bytes),next_offset:null}}));
 expect(openPreview).not.toHaveBeenCalled();expect(download).not.toHaveBeenCalled();expect(ui.queryByText('report.md')).toBeNull();
});
it.each([
 ['hidden','success'],['returned','success'],['hidden','failure'],['returned','failure'],
] as const)('returning from a child restores pagination and rejects a retired page settled while %s with %s',async(settle,result)=>{
 server=new Server();await server.attached('A');const cut=checkpoint(snapshot()).read_cut,target=server.target('A'),openPreview=vi.fn(),download=vi.fn();let owned=true;
 server.held.add('conversation/checkpointPage');
 const first={...head,total:'17'},source={client:server.client,target,cut:()=>cut,owned:()=>owned};
 const view=()=> <JournalSourceProvider source={source}><PreviewContext.Provider value={{openPreview,download}}><TurnDeliveries conversationId="conversation-A" batches={[{position:'2',head:first}]}/></PreviewContext.Provider></JournalSourceProvider>;
 const ui=render(view());
 const response=(offset:number,count:number,name:string):MethodResult=>({type:'checkpoint_page',page:{cursor:'0',read_cut:cut,domain:head.domain,total:'17',items:Array.from({length:count},(_,at)=>({offset:String(offset+at),identity:`delivery ${offset+at}`,value:{...item(offset+at),file:{...file,path:`out/${name}-${offset+at}.md`,name:`${name}-${offset+at}.md`},metadata_deferred:false},content:[],deferred:false})),next_offset:offset+count<17?String(offset+count):null,collections:[]}});
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Show more'})));
 const firstPage=await server.waitFor('conversation/checkpointPage',1);
 await act(async()=>server.socket.success(firstPage,response(8,8,'loaded')));
 expect(ui.getByRole('button',{name:'Show all 16 delivered files'})).toBeTruthy();
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Show more'})));
 const retired=await server.waitFor('conversation/checkpointPage',2);
 expect(retired).toMatchObject({params:{offset:16,cut}});
 const settleRetired=()=>result==='success'?server.socket.success(retired,response(16,1,'retired'))
  :server.socket.deliver({jsonrpc:'2.0',id:retired.id,error:{code:-32000,message:'Retired page failed',data:{kind:'invalid_state'}}});
 owned=false;ui.rerender(view());
 if(settle==='hidden')await act(async()=>settleRetired());
 owned=true;ui.rerender(view());
 // The parent keeps its loaded prefix, but does not keep the old read's busy state.
 expect((ui.getByRole('button',{name:'Show more'}) as HTMLButtonElement).disabled).toBe(false);
 expect(ui.getByRole('button',{name:'Show all 16 delivered files'})).toBeTruthy();
 if(settle==='returned')await act(async()=>settleRetired());
 expect((ui.getByRole('button',{name:'Show more'}) as HTMLButtonElement).disabled).toBe(false);
 expect(ui.queryByRole('button',{name:'Show all 17 delivered files'})).toBeNull();
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Show more'})));
 const fresh=await server.waitFor('conversation/checkpointPage',3);
 expect(fresh).toMatchObject({params:{offset:16,cut}});
 await act(async()=>server.socket.success(fresh,response(16,1,'current')));
 fireEvent.click(ui.getByRole('button',{name:'Show all 17 delivered files'}));
 expect(ui.getByRole('button',{name:'Preview current-16.md in sidebar'})).toBeTruthy();
 expect(ui.queryByText('retired-16.md')).toBeNull();
 expect(ui.queryByRole('button',{name:'Show more'})).toBeNull();
 expect(ui.queryByRole('alert')).toBeNull();
});
it('selected ranges reject oversized scalar evidence before reads and collection pages preserve exact u64 totals',async()=>{
 const cut=checkpoint(snapshot()).read_cut,read=vi.fn();
 await expect(readDeliveryMetadata({...item(0),encoded_bytes:'9'.repeat(1_000_000)},cut,read,()=>true)).rejects.toThrow('unavailable');expect(read).not.toHaveBeenCalled();
 const page:CheckpointPage={cursor:'0',read_cut:cut,domain:head.domain,total:'18446744073709551615',items:[{offset:'8',identity:'original',value:item(8),content:[],deferred:true}],next_offset:'9',collections:[]};
 const next=deliveryPage({...head,total:page.total},page,cut,8);
 expect(next.total).toBe('18446744073709551615');expect(next.next_offset).toBe(9);
 expect(()=>deliveryPage(head,{...page,read_cut:{...cut,conversation_id:'foreign'}},cut,8)).toThrow('exactly');
});
