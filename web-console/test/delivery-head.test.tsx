import {act,cleanup,fireEvent,render,waitFor} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import {DeliveryHead} from '../src/app/agent/DeliveryHead';
import {AgentTranscript} from '../src/app/agent/AgentTranscript';
import {JournalSourceProvider} from '../src/app/agent/ObservationSource';
import {PreviewContext} from '../src/app/components/ArtifactPreview';
import {PreviewWorkspaceOwner} from '../src/app/preview-workspace';
import {WorkspaceAuthority} from '../src/workspaces/authority';
import {checkpoint,Server,snapshot} from './fixture';
import type {CheckpointPage,RuntimeClientDeliveryHead,RuntimeClientDeliveryItem,RuntimeClientMessageDescriptor,RuntimeClientTranscriptEntry,RuntimeClientTranscriptPage,SessionFileReference} from '../../protocol/app-server/v57';
import {deliveryPage,readDeliveryMetadata} from '../../protocol/app-server/delivery-observation';
import {samePreviewSource} from '../src/client/session-files';
let server:Server;
afterEach(()=>{cleanup();server?.client.disconnect();});
const file:SessionFileReference={scope:{conversation_id:'original-inherited-owner',device:'1',inode:'2'},path:'out/report.md',name:'report.md',mime_type:'text/markdown',description:'native selected metadata'};
const locator={type:'by_ledger_position' as const,position:2};
const item=(index:number):RuntimeClientDeliveryItem=>({index,locator,file:null,metadata_deferred:true,encoded_bytes:String(new TextEncoder().encode(JSON.stringify(file)).length),metadata_domain:`messages.ledger:2.deliveries.${index}.metadata`});
const head:RuntimeClientDeliveryHead={domain:'messages.ledger:2.deliveries',total:'9',items:Array.from({length:8},(_,index)=>({...item(index),file,metadata_deferred:false})) as RuntimeClientDeliveryHead['items'],next_offset:8};
it('descriptor delivery previews retain exact occurrences through transcript hydration without merging other declarations',async()=>{
 server=new Server();await server.attached('A');
 const authority=new WorkspaceAuthority(server.workspaceHost);await authority.observe();
 const owner=new PreviewWorkspaceOwner(server.client,server.workspaceHost,authority),stop=owner.start();owner.selectSession('A');
 try{
  const base=snapshot(),cut=checkpoint(base).read_cut,files=[file,{...file,path:'other/report.md'}],messageId='tool-result';
  const descriptor:RuntimeClientMessageDescriptor={identity:messageId,locator,role:'tool',kind:'message',block_count:'0',tool_call_count:'0',tool_calls:[],content:[],collections:[],
   deliveries:{...head,total:'2',next_offset:null,items:files.map((file,index)=>({...item(index),file,metadata_deferred:false})) as RuntimeClientDeliveryHead['items']}};
  const entry=(id:string,cursor:string):RuntimeClientTranscriptEntry=>({cursor,item:{type:'message',message:{role:'tool',id,occurrence:{assistant_message_id:`call-${id}`,block_index:0},tool_call_id:`call-${id}`,tool_id:'deliver',result:{status:{type:'success'},duration_ms:'1',deliveries:files}}}});
  const source={client:server.client,target:server.target('A'),cut:()=>cut,owned:()=>true},intents={openPreview:owner.openPreview.bind(owner),download:owner.download.bind(owner)};
  const view=(page?:RuntimeClientTranscriptPage)=><JournalSourceProvider source={source}><PreviewContext.Provider value={intents}><AgentTranscript snapshot={{...base,message_descriptors:[descriptor]}} history={page&&{epoch:1,page}}/></PreviewContext.Provider></JournalSourceProvider>;
  const ui=render(view()),open=(index:number)=>fireEvent.click(ui.getAllByRole('button',{name:'Preview report.md in sidebar'})[index]);
  open(0);const first=owner.getSnapshot().workspace!.tabs[0].id;
  ui.rerender(view({entries:[entry(messageId,'3')]}));
  open(1);const occurrences=owner.getSnapshot().workspace!.tabs.map(tab=>tab.id);expect(occurrences).toHaveLength(2);
  open(0);expect(owner.getSnapshot().workspace!.tabs.map(tab=>tab.id)).toEqual(occurrences);
  expect(owner.getSnapshot().workspace!.panes[0].selected).toBe(first);
  ui.rerender(view({entries:[entry(messageId,'3'),entry('independent-result','4')]}));
  open(2);expect(owner.getSnapshot().workspace!.tabs).toHaveLength(3);
  expect(owner.getSnapshot().workspace!.tabs[2].artifact.source).toMatchObject({locator:{type:'by_message_id',message_id:'independent-result'},index:0,file});
 }finally{stop();}
});
it('same-file delivery indexes remain distinct and hydrate to the same occurrence',async()=>{
 server=new Server();await server.attached('A');
 const base=snapshot(),cut=checkpoint(base).read_cut,openPreview=vi.fn(),download=vi.fn(),messageIdentity='same-file-result';
 const descriptor:RuntimeClientMessageDescriptor={identity:messageIdentity,locator,role:'tool',kind:'message',block_count:'0',tool_call_count:'0',tool_calls:[],content:[],collections:[],
  deliveries:{...head,total:'2',next_offset:null,items:[{...item(0),file,metadata_deferred:false},{...item(1),file,metadata_deferred:false}] as RuntimeClientDeliveryHead['items']}};
 const hydrated:RuntimeClientTranscriptEntry={cursor:'2',item:{type:'message',message:{role:'tool',id:messageIdentity,occurrence:{assistant_message_id:'assistant',block_index:0},tool_call_id:'present',tool_id:'present',result:{status:{type:'success'},deliveries:[file,file],duration_ms:'0'}}}};
 const source={client:server.client,target:server.target('A'),cut:()=>cut,owned:()=>true};
 const view=(page?:RuntimeClientTranscriptPage)=><JournalSourceProvider source={source}><PreviewContext.Provider value={{openPreview,download}}><AgentTranscript snapshot={{...base,message_descriptors:[descriptor]}} history={page&&{epoch:1,page}}/></PreviewContext.Provider></JournalSourceProvider>;
 const ui=render(view()),open=(index:number)=>fireEvent.click(ui.getAllByRole('button',{name:'Preview report.md in sidebar'})[index]);
 open(0);open(1);
 const first=openPreview.mock.calls[0][0].source,second=openPreview.mock.calls[1][0].source;
 expect(first.locator).toEqual({type:'by_message_id',message_id:messageIdentity});
 expect(samePreviewSource(first,second)).toBe(false);
 ui.rerender(view({entries:[hydrated]}));
 open(0);
 expect(samePreviewSource(first,openPreview.mock.calls[2][0].source)).toBe(true);
});
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
 const ui=render(<JournalSourceProvider source={source}><PreviewContext.Provider value={{openPreview,download}}><DeliveryHead head={head} messageIdentity={messageIdentity}/></PreviewContext.Provider></JournalSourceProvider>);
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
 const ui=render(<JournalSourceProvider source={{client:server.client,target,cut:()=>cut,owned:()=>owned}}><PreviewContext.Provider value={{openPreview,download}}><DeliveryHead head={{...head,items:[selected],next_offset:null}} messageIdentity={null}/></PreviewContext.Provider></JournalSourceProvider>);
 expect(ui.queryByRole('button',{name:'Preview 1/9'})).toBeNull();
 const request=await server.waitFor('conversation/checkpointValueRead',1);owned=false;
 await act(async()=>server.socket.success(request,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut,domain:selected.metadata_domain,item_offset:0,path:[],offset:0,bytes_base64:btoa(JSON.stringify(file)),readable_end:Number(selected.encoded_bytes),next_offset:null}}));
 expect(openPreview).not.toHaveBeenCalled();expect(download).not.toHaveBeenCalled();expect(ui.queryByText('report.md')).toBeNull();
});
it('selected ranges reject oversized scalar evidence before reads and collection pages preserve exact u64 totals',async()=>{
 const cut=checkpoint(snapshot()).read_cut,read=vi.fn();
 await expect(readDeliveryMetadata({...item(0),encoded_bytes:'9'.repeat(1_000_000)},cut,read,()=>true)).rejects.toThrow('unavailable');expect(read).not.toHaveBeenCalled();
 const page:CheckpointPage={cursor:'0',read_cut:cut,domain:head.domain,total:'18446744073709551615',items:[{offset:'8',identity:'original',value:item(8),content:[],deferred:true}],next_offset:'9',collections:[]};
 const next=deliveryPage({...head,total:page.total},page,cut,8);
 expect(next.total).toBe('18446744073709551615');expect(next.next_offset).toBe(9);
 expect(()=>deliveryPage(head,{...page,read_cut:{...cut,conversation_id:'foreign'}},cut,8)).toThrow('exactly');
});
