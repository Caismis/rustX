import { cleanup, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { CheckpointPage, MessageBlock, RuntimeClientMessageDescriptor, UserContentBlock } from '../../protocol/app-server/v51';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import { Server, checkpoint, snapshot, readingWindow } from './fixture';
let server: Server;
afterEach(()=>{cleanup();server?.client.disconnect();});

it.each(['live','history','child','descriptor'] as const)('%s Chat reads the text after 32 attachments into the normal message view',async mode=>{
 server=new Server();await server.attached('A');
 const base=snapshot(),native=readingWindow({}).page.source_cut ?? { read:readingWindow({}).cut,ledger_through:'70',transcript_total:'70',surface_revision:'0',inherited_through:'0' };
 const cut={...checkpoint(base).read_cut,source_cut:native};
 const blocks:UserContentBlock[]=[...Array.from({length:32},(_,n)=>({type:'uploaded_file' as const,batch_id:`batch-${n}`,name:`file-${n}.txt`})),{type:'text',text:'The actual instruction after all attachments'}];
 const message:MessageBlock={role:'user',source:'human',kind:'message',id:'upload-message',content:blocks.slice(0,32)};
 const frontier={domain:'messages.ledger:17.blocks',revision:'1',total:'33',loaded:'32',next_offset:'32'};
 const descriptor:RuntimeClientMessageDescriptor={identity:message.id,locator:{type:'by_ledger_position',position:17},role:'user',kind:'message',block_count:'33',tool_call_count:'0',tool_calls:[],content:[],collections:[frontier]};
 const page={source_cut:native,entries:mode==='descriptor'?[]:[{cursor:'17',item:{type:'message' as const,message}}],collections:[frontier],deferred_entries:mode==='descriptor'?[{offset:'17',identity:message.id,value:descriptor,deferred:true,content:[]}]:[]};
 const method=mode==='history'?'session/historyPage':'conversation/checkpointPage';
 server.handlers.set(method,request=>{
  if(request.method!=='session/historyPage'&&request.method!=='conversation/checkpointPage')throw Error('page');
  const offset=request.params.offset;
  expect(request.params.domain).toBe(mode==='descriptor'?'messages.ledger:17.blocks':'messages.message:upload-message.blocks');
  if(mode==='child')expect(request.params).toMatchObject({agent_id:'worker'});
  const items=blocks.slice(offset,offset+32).map((value,index)=>({offset:String(offset+index),identity:value.type,value,content:[],deferred:false}));
  return {type:'checkpoint_page',page:{read_cut:cut,cursor:cut.revision,domain:request.params.domain,total:'33',items,next_offset:offset+items.length<33?String(offset+items.length):null,collections:[]}};
 });
 const current={...base,messages:mode==='descriptor'?[]:[message],message_descriptors:mode==='history'?[]:[descriptor],transcript:page};
 const ui=render(<JournalSourceProvider source={mode==='history'?{client:server.client,sessionId:'A',historyCut:native,cut:()=>undefined,owned:()=>true}:{client:server.client,target:server.target('A'),agentId:mode==='child'?'worker':undefined,cut:()=>cut,owned:()=>true}} messages={current.messages} transcript={page} history={page}><AgentTranscript snapshot={current} history={mode==='history'?{epoch:1,page}:undefined}/></JournalSourceProvider>);
 await ui.findByText('The actual instruction after all attachments');
 expect(ui.getAllByText(/file-\d+\.txt/)).toHaveLength(32);
 expect(ui.queryByRole('alert')).toBeNull();
 const reads=server.requests.filter(row=>row.request.method===method);
 expect(reads.map(row=>('offset' in row.request.params?row.request.params.offset:undefined))).toEqual(mode==='descriptor'?[0,32]:[32]);
});

it.each(['live','history'] as const)('%s assistant blocks after the header continue across multiple native pages in original order',async mode=>{
 server=new Server();await server.attached('A');const base=snapshot(),history={read:readingWindow({}).cut,ledger_through:'70',transcript_total:'70',surface_revision:'0',inherited_through:'0'},cut={...checkpoint(base).read_cut,source_cut:history};
 const blocks=Array.from({length:98},(_,n)=>({type:'text' as const,text:`Original paragraph ${n}`}));
 const message:MessageBlock={id:'long-assistant',role:'assistant',content:blocks.slice(0,32)};
 const descriptor:RuntimeClientMessageDescriptor={identity:message.id,locator:{type:'by_ledger_position',position:17},role:'assistant',kind:'message',block_count:'98',tool_call_count:'0',tool_calls:[],content:[],collections:[{domain:'messages.ledger:17.blocks',revision:'1',total:'98',loaded:'32',next_offset:'32'}]};
 const method=mode==='history'?'session/historyPage':'conversation/checkpointPage';
 server.handlers.set(method,request=>{
  if(request.method!=='conversation/checkpointPage'&&request.method!=='session/historyPage')throw Error('page');const offset=request.params.offset,items=blocks.slice(offset,offset+32).map((value,index)=>({offset:String(offset+index),identity:value.type,value,content:[],deferred:false}));
  const page:CheckpointPage={read_cut:cut,cursor:cut.revision,domain:request.params.domain,total:'98',items,next_offset:offset+items.length<98?String(offset+items.length):null,collections:[]};
  return {type:'checkpoint_page',page};
 });
 const page={source_cut:history,entries:[{cursor:'17',item:{type:'message' as const,message}}],collections:descriptor.collections};
 const ui=render(<JournalSourceProvider source={mode==='history'?{client:server.client,sessionId:'A',historyCut:history,cut:()=>undefined,owned:()=>true}:{client:server.client,target:server.target('A'),cut:()=>cut,owned:()=>true}} messages={[message]} transcript={page}><AgentTranscript snapshot={{...base,messages:[message],message_descriptors:mode==='history'?[]:[descriptor],transcript:page}} history={mode==='history'?{epoch:1,page}:undefined}/></JournalSourceProvider>);
 await ui.findByText('Original paragraph 97');
 const content=ui.container.textContent!;for(let n=32;n<97;n++)expect(content.indexOf(`Original paragraph ${n}`)).toBeLessThan(content.indexOf(`Original paragraph ${n+1}`));
 expect(server.requests.filter(row=>row.request.method===method).map(row=>row.request.params)).toMatchObject([{offset:32},{offset:64},{offset:96}]);
});

it('an expired live cut retries on the next native checkpoint and retains the successful immutable body',async()=>{
 const {act,waitFor}=await import('@testing-library/react');
 server=new Server();await server.attached('A');const base=snapshot();let cut=checkpoint(base).read_cut;
 const message:MessageBlock={id:'admitted',role:'user',source:'human',content:Array.from({length:32},(_,n)=>({type:'uploaded_file' as const,batch_id:'batch',name:`file-${n}.txt`}))};
 const descriptor:RuntimeClientMessageDescriptor={identity:message.id,locator:{type:'by_ledger_position',position:17},role:'user',kind:'message',block_count:'33',tool_call_count:'0',tool_calls:[],content:[],collections:[{domain:'messages.ledger:17.blocks',revision:'1',total:'33',loaded:'32',next_offset:'32'}]};
 const current={...base,messages:[message],message_descriptors:[descriptor],transcript:{entries:[{cursor:'17',item:{type:'message' as const,message}}]}};
 server.held.add('conversation/checkpointPage');
 const source={client:server.client,target:server.target('A'),cut:()=>cut,owned:()=>true};
 const view=()=> <JournalSourceProvider source={source} messages={[message]} transcript={current.transcript}><AgentTranscript snapshot={current}/></JournalSourceProvider>;
 const ui=render(view()),expired=await server.waitFor('conversation/checkpointPage',1);
 await act(async()=>server.socket.deliver({jsonrpc:'2.0',id:expired.id,error:{code:-32000,message:'read_cut_expired'}}));
 await ui.findByRole('alert');expect(server.requests.filter(row=>row.request.method==='conversation/checkpointPage')).toHaveLength(1);
 cut={...cut,revision:'1'};ui.rerender(view());
 const retry=await server.waitFor('conversation/checkpointPage',2);
 expect(retry.params).toMatchObject({cut,offset:32});
 await act(async()=>server.socket.success(retry,{type:'checkpoint_page',page:{read_cut:cut,cursor:cut.revision,domain:'messages.message:admitted.blocks',total:'33',next_offset:null,collections:[],items:[{offset:'32',identity:'text',value:{type:'text',text:'Recovered instruction'},content:[],deferred:false}]}}));
 const body=await ui.findByText('Recovered instruction');await waitFor(()=>expect(ui.queryByRole('alert')).toBeNull());
 cut={...cut,revision:'2'};ui.rerender(view());expect(ui.getByText('Recovered instruction')).toBe(body);
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointPage')).toHaveLength(2);
});
