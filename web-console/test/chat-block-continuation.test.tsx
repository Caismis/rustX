import { cleanup, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { CheckpointPage, MessageBlock, RuntimeClientMessageDescriptor, UserContentBlock } from '../../protocol/app-server/v57';
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

it.each([false,true])('an expired live cut retries on the next checkpoint and retains its body alongside frozen history: %s',async frozen=>{
 const {act,waitFor}=await import('@testing-library/react');
 server=new Server();await server.attached('A');const base=snapshot();let cut=checkpoint(base).read_cut;
 const message:MessageBlock={id:'admitted',role:'user',source:'human',content:Array.from({length:32},(_,n)=>({type:'uploaded_file' as const,batch_id:'batch',name:`file-${n}.txt`}))};
 const descriptor:RuntimeClientMessageDescriptor={identity:message.id,locator:{type:'by_ledger_position',position:17},role:'user',kind:'message',block_count:'33',tool_call_count:'0',tool_calls:[],content:[],collections:[{domain:'messages.ledger:17.blocks',revision:'1',total:'33',loaded:'32',next_offset:'32'}]};
 const current={...base,messages:[message],message_descriptors:[descriptor],transcript:{entries:[{cursor:'17',item:{type:'message' as const,message}}]}};
 server.held.add('conversation/checkpointPage');
 const source={client:server.client,target:server.target('A'),cut:()=>cut,owned:()=>true};
 const history={source_cut:{read:readingWindow({}).cut,ledger_through:'0',transcript_total:'0',surface_revision:'0',inherited_through:'0'},entries:[{cursor:'1',item:{type:'message' as const,message:{id:'older-frozen-message',role:'assistant' as const,content:[{type:'text' as const,text:'Preserved historical body'}]}}}]};
 const view=()=> <JournalSourceProvider source={source} historical={frozen} messages={[message]} transcript={current.transcript} history={frozen?history:undefined}><AgentTranscript snapshot={current} history={frozen?{epoch:1,page:history,window:{cut:history.source_cut.read,page:history,newer_cursor:'1'}}:undefined}/></JournalSourceProvider>;
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

it.each(['hidden','reopened'] as const)('child continuation resumes after Chat reopens and fences the old response completed while %s',async completion=>{
 const {act,waitFor}=await import('@testing-library/react');
 server=new Server();await server.attached('A');const base=snapshot(),cut={...checkpoint(base).read_cut,activation_id:'worker-activation'};
 const message:MessageBlock={id:'child-instruction',role:'user',source:{agent:{agent_id:'parent'}},content:Array.from({length:32},(_,n)=>({type:'uploaded_file',batch_id:'child-batch',name:`child-file-${n}.txt`}))};
 const descriptor:RuntimeClientMessageDescriptor={identity:message.id,locator:{type:'by_ledger_position',position:17},role:'user',kind:'message',block_count:'33',tool_call_count:'0',tool_calls:[],content:[],collections:[{domain:'messages.ledger:17.blocks',revision:'1',total:'33',loaded:'32',next_offset:'32'}]};
 const current={...base,messages:[message],message_descriptors:[descriptor],transcript:{entries:[{cursor:'17',item:{type:'message' as const,message}}]}};
 let active=true;const source={client:server.client,target:server.target('A'),agentId:'worker',cut:()=>cut,owned:()=>active};
 const view=()=> <div hidden={!active}><JournalSourceProvider source={source} messages={[message]} transcript={current.transcript}><AgentTranscript snapshot={current}/></JournalSourceProvider></div>;
 const answer=(request:Awaited<ReturnType<Server['waitFor']>>,text:string)=>server.socket.success(request,{type:'checkpoint_page',page:{read_cut:cut,cursor:cut.revision,domain:'messages.message:child-instruction.blocks',total:'33',next_offset:null,collections:[],items:[{offset:'32',identity:'text',value:{type:'text',text},content:[],deferred:false}]}});
 server.held.add('conversation/checkpointPage');
 const ui=render(view()),first=await server.waitFor('conversation/checkpointPage',1);
 active=false;ui.rerender(view());
 if(completion==='hidden')await act(async()=>answer(first,'Discarded response'));
 active=true;ui.rerender(view());
 await waitFor(()=>expect(server.requests.filter(row=>row.request.method==='conversation/checkpointPage')).toHaveLength(2));
 const second=await server.waitFor('conversation/checkpointPage',2);
 if(completion==='reopened')await act(async()=>answer(first,'Discarded response'));
 expect(ui.queryByText('Discarded response')).toBeNull();
 await act(async()=>answer(second,'Complete child instruction'));
 const body=await ui.findByText('Complete child instruction');
 active=false;ui.rerender(view());active=true;ui.rerender(view());
 expect(ui.getByText('Complete child instruction')).toBe(body);
 expect(ui.queryByRole('alert')).toBeNull();
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointPage')).toHaveLength(2);
});

it.each([false,true])('frozen child continuation keeps C0 when the independent live cut advances: %s',async advance=>{
 server=new Server();await server.attached('A');const base=snapshot(),history={read:{...readingWindow({}).cut,ledger_through:'70',transcript_total:'70'},ledger_through:'70',transcript_total:'70',surface_revision:'0',inherited_through:'0'};
 const live={...checkpoint(base).read_cut,conversation_id:history.read.conversation_id,activation_id:'worker-activation',source_cut:advance?{...history,read:{...history.read,journal:'1001',ledger_through:'99'},ledger_through:'99'}:history};
 const message:MessageBlock={id:'frozen-child-message',role:'assistant',content:Array.from({length:32},(_,n)=>({type:'text',text:`Historical paragraph ${n}`}))};
 const frontier={domain:'messages.ledger:17.blocks',revision:'1',total:'33',loaded:'32',next_offset:'32'};
 const page={source_cut:history,entries:[{cursor:'17',item:{type:'message' as const,message}}],collections:[frontier]};
 server.handlers.set('conversation/checkpointPage',request=>{
  if(request.method!=='conversation/checkpointPage')throw Error('page');
  expect(request.params).toMatchObject({agent_id:'worker',cut:{conversation_id:history.read.conversation_id,activation_id:'worker-activation',revision:'0',through:history.read.journal,source_cut:history,domain_revisions:{},collection_cuts:{}},offset:32});
  return {type:'checkpoint_page',page:{read_cut:{canonical_foreground:null,live_publication:null,...request.params.cut},cursor:'0',domain:request.params.domain,total:'33',next_offset:null,collections:[],items:[{offset:'32',identity:'text',value:{type:'text',text:'Original C0 historical suffix'},content:[],deferred:false}]}};
 });
 const ui=render(<JournalSourceProvider historical source={{client:server.client,target:server.target('A'),agentId:'worker',cut:()=>live,owned:()=>true}} messages={[message]} transcript={page} history={page}><AgentTranscript snapshot={{...base,messages:[message],transcript:page}} history={{epoch:1,page}}/></JournalSourceProvider>);
 await ui.findByText('Original C0 historical suffix');
 expect(ui.queryByRole('alert')).toBeNull();
 expect(server.requests.filter(row=>row.request.method==='conversation/checkpointPage')).toHaveLength(1);
 expect(server.requests.filter(row=>row.request.method==='session/historyPage')).toHaveLength(0);
});

it.each([1,33])('a frozen child window with %i old blocks keeps live-tail continuations on their own checkpoint',async oldCount=>{
 server=new Server();await server.attached('A');const base=snapshot();
 const native={read:{...readingWindow({}).cut,ledger_through:'70',transcript_total:'70'},ledger_through:'70',transcript_total:'70',surface_revision:'0',inherited_through:'0'};
 const liveCut={...checkpoint(base).read_cut,conversation_id:native.read.conversation_id,activation_id:'worker-activation',revision:'2',through:'1001',source_cut:{...native,ledger_through:'71',transcript_total:'71',read:{...native.read,journal:'1001',transcript:'1001',ledger_through:'71',transcript_total:'71'}}};
 const old:MessageBlock={id:'old-window-message',role:'assistant',content:Array.from({length:Math.min(32,oldCount)},(_,n)=>({type:'text',text:`Old paragraph ${n}`}))};
 const recent:MessageBlock={id:'new-tail-message',role:'assistant',content:Array.from({length:32},(_,n)=>({type:'text',text:`Recent paragraph ${n}`}))};
 const descriptor:RuntimeClientMessageDescriptor={identity:recent.id,locator:{type:'by_ledger_position',position:71},role:'assistant',kind:'message',block_count:'33',tool_call_count:'0',tool_calls:[],content:[],collections:[{domain:'messages.ledger:71.blocks',revision:'2',total:'33',loaded:'32',next_offset:'32'}]};
 const page={source_cut:native,entries:[{cursor:'17',item:{type:'message' as const,message:old}}],collections:oldCount===33?[{domain:'messages.ledger:17.blocks',revision:'1',total:'33',loaded:'32',next_offset:'32'}]:[]};
 server.handlers.set('conversation/checkpointPage',request=>{
  if(request.method!=='conversation/checkpointPage')throw Error('page');const past=request.params.domain.includes(old.id);
  const cut=past?{conversation_id:native.read.conversation_id,activation_id:'worker-activation',revision:'0',through:native.read.journal,source_cut:native,domain_revisions:{},collection_cuts:{}}:liveCut;
  return {type:'checkpoint_page',page:{read_cut:{canonical_foreground:null,live_publication:null,...cut},cursor:cut.revision,domain:request.params.domain,total:'33',next_offset:null,collections:[],items:[{offset:'32',identity:'text',value:{type:'text',text:past?'Original historical ending':'Independent live ending'},content:[],deferred:false}]}};
 });
 const ui=render(<JournalSourceProvider historical source={{client:server.client,target:server.target('A'),agentId:'worker',cut:()=>liveCut,owned:()=>true}} messages={[recent]} transcript={{entries:[{cursor:'1001',item:{type:'message',message:recent}}]}} history={page}><AgentTranscript snapshot={{...base,messages:[recent],message_descriptors:[descriptor],transcript:{entries:[{cursor:'1001',item:{type:'message',message:recent}}]}}} history={{epoch:1,page,window:{cut:native.read,page,newer_cursor:'17'}}}/></JournalSourceProvider>);
 await ui.findByText('Independent live ending');
 if(oldCount===33)await ui.findByText('Original historical ending');
 expect(ui.queryByRole('alert')).toBeNull();
 const reads=server.requests.flatMap(({request})=>request.method==='conversation/checkpointPage'?[request]:[]);
 expect(reads).toHaveLength(oldCount===33?2:1);
 expect(reads.find(request=>request.params.domain.includes(recent.id))?.params.cut).toEqual(liveCut);
 if(oldCount===33)expect(reads.find(request=>request.params.domain.includes(old.id))?.params.cut.source_cut).toEqual(native);
});
