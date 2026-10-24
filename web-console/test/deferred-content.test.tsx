import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { SourceContent } from '../src/app/agent/SourceContent';
import type { ContentBinding, ContentChunk, ContentRef } from '../../protocol/app-server/v57';
afterEach(()=>{cleanup();vi.restoreAllMocks();});
const reference=(id:string,bytes:number):ContentRef=>({conversation_id:'conversation-A',source:{type:'live_publication_prefix',component:'payload',stream_id:'stream-A',message_id:id,attempt_id:'attempt-A',turn_id:'turn-A',publication_request_id:'request-A',block_index:0},representation:'utf8_text',version:1,content_revision:1,readable_end:bytes});
const binding=(ref:ContentRef):ContentBinding=>({path:['messages','0','content','0','text'],content_ref:ref,deferred:true});
const chunk=(ref:ContentRef,bytes:Uint8Array,offset=0):ContentChunk=>({content_ref:ref,offset,end_offset:offset+bytes.length,readable_end:ref.readable_end,data_base64:btoa(Array.from(bytes,b=>String.fromCharCode(b)).join('')),next_offset:offset+bytes.length<ref.readable_end?offset+bytes.length:null,complete:offset+bytes.length===ref.readable_end});
it('automatically assembles split UTF-8 chunks without exposing byte-reading controls',async()=>{
 const bytes=new TextEncoder().encode('原始内容完整显示'),ref=reference('text',bytes.length),read=vi.fn(async(_ref:ContentRef,offset:number)=>chunk(ref,bytes.slice(offset,offset===0?2:bytes.length),offset));
 const ui=render(<SourceContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);
 await waitFor(()=>expect(ui.container.querySelector('pre')?.textContent).toBe('原始内容完整显示'));
 expect(read.mock.calls.map(call=>call[1])).toEqual([0,2]);expect(ui.queryAllByRole('button')).toHaveLength(0);
});
it('replacing content rejects a physically late reply from the old identity',async()=>{
 const old=reference('old',3),next=reference('next',3);let finish!:(chunk:ContentChunk)=>void,owns!:()=>boolean;
 const read=(ref:ContentRef,_offset:number,current:()=>boolean)=>ref===old?(owns=current,new Promise<ContentChunk>(resolve=>{finish=resolve;})):Promise.resolve(chunk(next,new TextEncoder().encode('new')));
 const ui=render(<SourceContent bindings={[binding(old)]} owned={()=>true} read={read}/>);ui.rerender(<SourceContent bindings={[binding(next)]} owned={()=>true} read={read}/>);
 expect(owns()).toBe(false);await act(async()=>finish(chunk(old,new TextEncoder().encode('old'))));await ui.findByText('new');expect(ui.queryByText('old')).toBeNull();
});
it('same-source callback changes retain an in-flight read; revoked ownership fences it',async()=>{
 const ref=reference('same',3);let finish!:(chunk:ContentChunk)=>void,owns!:()=>boolean;
 const read=vi.fn((_ref:ContentRef,_offset:number,current:()=>boolean)=>{owns=current;return new Promise<ContentChunk>(resolve=>{finish=resolve;});});
 const ui=render(<SourceContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);ui.rerender(<SourceContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);expect(owns()).toBe(true);expect(read).toHaveBeenCalledTimes(1);
 ui.rerender(<SourceContent bindings={[binding(ref)]} owned={()=>false} read={read}/>);expect(owns()).toBe(false);await act(async()=>finish(chunk(ref,new TextEncoder().encode('old'))));expect(ui.queryByText('old')).toBeNull();
});
it('preserves binary bytes and opaque JSON exactly',async()=>{
 for(const [bytes,representation] of [[Uint8Array.of(0,255,128,10,65,0),'binary_bytes'],[new TextEncoder().encode('{ "wide": 9007199254740993, "escape": "\\u0061" }'),'canonical_json_v1']] as const){
 const ref={...reference(representation,bytes.length),representation};const ui=render(<SourceContent bindings={[binding(ref)]} owned={()=>true} read={async()=>chunk(ref,bytes)}/>);
 await waitFor(()=>expect(ui.container.querySelector('pre')?.textContent).toBe(representation==='binary_bytes'?btoa(Array.from(bytes,b=>String.fromCharCode(b)).join('')):new TextDecoder().decode(bytes)));ui.unmount();}
});
it('rejects a source substitution without publishing its bytes',async()=>{
 const ref=reference('expected',3),other=reference('other',3);
 const ui=render(<SourceContent bindings={[binding(ref)]} owned={()=>true} read={async()=>chunk(other,new TextEncoder().encode('bad'))}/>);
 await ui.findByRole('alert');expect(ui.queryByText('bad')).toBeNull();
});

it('a native immutable audit keeps the exact already loaded publication without rereading expired live bytes', async () => {
 const live = reference('partial', 3);
 if(live.source.type!=='live_publication_prefix')throw Error('live source');
 const frozen:ContentRef = {...live,source:{...live.source,type:'durable_transcript_audit'}};
 const read=vi.fn(async(ref:ContentRef)=>chunk(ref,new TextEncoder().encode('old')));
 const ui=render(<SourceContent bindings={[binding(live)]} owned={()=>true} read={read}/>);
 const body=await ui.findByText('old');
 ui.rerender(<SourceContent bindings={[binding(frozen)]} owned={()=>true} read={read}/>);
 expect(ui.getByText('old')).toBe(body);expect(read).toHaveBeenCalledTimes(1);
});

it.each(['stream','request','length','conversation'] as const)('an audit with different %s cannot reuse loaded live bytes', async change => {
 const live=reference('partial',3);
 if(live.source.type!=='live_publication_prefix')throw Error('live source');
 const frozen:ContentRef={...live,conversation_id:change==='conversation'?'another':live.conversation_id,readable_end:change==='length'?4:3,
   source:{...live.source,type:'durable_transcript_audit',stream_id:change==='stream'?'another':live.source.stream_id,publication_request_id:change==='request'?'another':live.source.publication_request_id}};
 const text=change==='length'?'news':'new';
 const read=vi.fn(async(ref:ContentRef)=>chunk(ref,new TextEncoder().encode(ref.source.type==='live_publication_prefix'?'old':text)));
 const ui=render(<SourceContent bindings={[binding(live)]} owned={()=>true} read={read}/>);await ui.findByText('old');
 ui.rerender(<SourceContent bindings={[binding(frozen)]} owned={()=>true} read={read}/>);
 await ui.findByText(text);expect(read).toHaveBeenCalledTimes(2);expect(ui.queryByText('old')).toBeNull();
});

it('a finite browser body reads the missing native bytes even when the native header was inline', async () => {
 const {Server}=await import('./fixture'),{JournalSourceProvider}=await import('../src/app/agent/ObservationSource'),{Message}=await import('../src/app/agent/Message');
 const server=new Server();await server.attached('A');
 try {
  const bytes=new TextEncoder().encode('Full native body'),ref=reference('large',bytes.length),data={...binding(ref),deferred:false};
  server.handlers.set('conversation/contentRead',()=>({type:'content_chunk',chunk:chunk(ref,bytes)}));
  const ui=render(<JournalSourceProvider source={{client:server.client,target:server.target('A'),cut:()=>undefined,owned:()=>true}} content={[data]}><Message message={{role:'assistant',id:'large',content:[{type:'text',text:''}]}}/></JournalSourceProvider>);
  await ui.findByText('Full native body');
 }finally{cleanup();server.client.disconnect();}
});

it('a stale local binding path cannot override an explicit native message identity', async () => {
 const {Server}=await import('./fixture'),{JournalSourceProvider}=await import('../src/app/agent/ObservationSource'),{Message}=await import('../src/app/agent/Message');
 const server=new Server();await server.attached('A');
 try {
  const message={role:'assistant' as const,id:'current',content:[{type:'text' as const,text:'Current body'}]},data={...binding(reference('other',30)),deferred:false};
  const ui=render(<JournalSourceProvider source={{client:server.client,target:server.target('A'),cut:()=>undefined,owned:()=>true}} content={[data]} messages={[message]}><Message message={message}/></JournalSourceProvider>);
  expect(ui.getByText('Current body')).toBeTruthy();
  expect(server.requests.some(row=>row.request.method==='conversation/contentRead')).toBe(false);
 }finally{cleanup();server.client.disconnect();}
});

it('partial-response copy reads only the exact immutable audit stream and omits reasoning', async () => {
 const {Server}=await import('./fixture'),{JournalSourceProvider}=await import('../src/app/agent/ObservationSource'),{CopyMessage}=await import('../src/app/agent/TurnTail');
 const copied=vi.spyOn(await import('../src/presentation/primitives/clipboard'),'writeClipboard').mockResolvedValue(true);
 const server=new Server();await server.attached('A');
 try {
  const text='Complete frozen body',bytes=new TextEncoder().encode(text),live=reference('partial',bytes.length);
  if(live.source.type!=='live_publication_prefix')throw Error('live source');
  const ref:ContentRef={...live,source:{...live.source,type:'durable_transcript_audit'}};
  const wrong:ContentRef={...ref,source:{...live.source,type:'durable_transcript_audit',stream_id:'another-stream'}};
  server.handlers.set('conversation/contentRead',request=>{if(request.method!=='conversation/contentRead')throw Error('method');expect(request.params.content_ref).toEqual(ref);return {type:'content_chunk',chunk:chunk(ref,bytes)};});
  const publication={stream_id:'stream-A',attempt_id:'attempt-A',turn_id:'turn-A',request_id:'request-A',message_id:'partial',kind:'incomplete' as const,settled_at:'2026-10-20T00:00:00Z',content:[{kind:'text' as const,block_index:0,text:''},{kind:'reasoning' as const,block_index:1,text:'Private reasoning'}]};
  const ui=render(<JournalSourceProvider source={{client:server.client,target:server.target('A'),cut:()=>undefined,owned:()=>true}} content={[binding(wrong),binding(ref)]}><CopyMessage text="" publication={publication}/></JournalSourceProvider>);
  fireEvent.click(ui.getByRole('button',{name:'Copy'}));await waitFor(()=>expect(copied).toHaveBeenCalledWith(text));
 }finally{cleanup();server.client.disconnect();}
});

it.each(['live','child','history'] as const)('%s messages fill their existing body automatically and never render metadata readers',async kind=>{
 const {Server,readCut}=await import('./fixture');
 const {JournalSourceProvider}=await import('../src/app/agent/ObservationSource');
 const {Message}=await import('../src/app/agent/Message');
 const {CopyMessage}=await import('../src/app/agent/TurnTail');
 const clipboard=await import('../src/presentation/primitives/clipboard');
 const copied=vi.spyOn(clipboard,'writeClipboard').mockResolvedValue(true);
 const server=new Server();if(kind==='history')await server.connect();else await server.attached('A');
 try {
  const text='正文自动补全：你好🙂',bytes=new TextEncoder().encode(text),id='original-message',base=reference(id,bytes.length);
  const ref:ContentRef=kind==='history'?{...base,source:{type:'canonical_message_block',component:'payload',locator:{type:'by_ledger_position',position:17},block_index:0}}:base;
  const data=binding(ref);if(kind==='history')data.path=['transcript','entries','0','item','message','content','0','text'];
  const message={id,role:'assistant' as const,content:[{type:'text' as const,text:'正文预览'}]},cut={read:readCut(),ledger_through:'17',transcript_total:'1',surface_revision:'0',inherited_through:'0'};
  const metadata={...data,content_ref:{...ref,representation:'canonical_json_v1' as const,readable_end:16384,source:{...ref.source,component:'block_metadata' as const}}};
  server.handlers.set(kind==='history'?'session/historyContentRead':'conversation/contentRead',request=>{
   if(request.method!=='session/historyContentRead'&&request.method!=='conversation/contentRead')throw Error('method');
   expect(request.params.content_ref).toEqual(ref);expect(request.params.max_bytes).toBe(65536);
   if(request.method==='session/historyContentRead')expect(request.params.cut).toEqual(cut);else expect(request.params.agent_id).toBe(kind==='child'?'child':null);
   return {type:'content_chunk',chunk:chunk(ref,bytes.slice(request.params.offset,request.params.offset===0?2:bytes.length),request.params.offset)};
  });
  const source=kind==='history'?{client:server.client,sessionId:'A',nodeId:'original-node',historyCut:cut,cut:()=>undefined,owned:()=>true}:{client:server.client,target:server.target('A'),agentId:kind==='child'?'child':undefined,cut:()=>undefined,owned:()=>true};
  const ui=render(<JournalSourceProvider source={source} content={[data,metadata]} messages={[message]} transcript={{entries:[{cursor:'1',item:{type:'message',message}}]}}><Message message={message}/><CopyMessage text="正文预览" messageId={message.id}/></JournalSourceProvider>);
  await ui.findByText(text);expect(ui.queryByText('正文预览')).toBeNull();expect(ui.queryByText(/16384|Read content|内容延迟加载|Content deferred/)).toBeNull();expect(ui.queryByRole('button',{name:/Read content|Read original/})).toBeNull();
  const reads=server.requests.filter(row=>row.request.method=== (kind==='history'?'session/historyContentRead':'conversation/contentRead'));expect(reads).toHaveLength(2);
  fireEvent.click(ui.getByRole('button',{name:'Copy'}));await waitFor(()=>expect(copied).toHaveBeenCalledWith(text));
  if(kind==='history')expect(server.requests.some(row=>row.request.method==='session/attach')).toBe(false);
 }finally{cleanup();server.client.disconnect();}
});

it('historical and live rows with the same local index retain separate canonical body sources',async()=>{
 const {Server}=await import('./fixture'),{JournalSourceProvider}=await import('../src/app/agent/ObservationSource'),{Message}=await import('../src/app/agent/Message');
 const server=new Server();await server.attached('A');
 try {
  const text=['Original history body','Original live body'];
  const messages=text.map((_,index)=>({id:`message-${index}`,role:'assistant' as const,content:[{type:'text' as const,text:`preview-${index}`}]}));
  const refs=text.map((value,index):ContentRef=>({...reference(`message-${index}`,new TextEncoder().encode(value).length),source:{type:'canonical_message_block',component:'payload',locator:{type:'by_ledger_position',position:index+1},block_index:0}}));
  const bindings=refs.map(ref=>({...binding(ref),path:['transcript','entries','0','item','message','content','0','text']}));
  const pages=messages.map((message,index)=>({entries:[{cursor:String(index+1),item:{type:'message' as const,message}}],content:[bindings[index]]}));
  server.handlers.set('conversation/contentRead',request=>{
   if(request.method!=='conversation/contentRead')throw Error('method');const ref=request.params.content_ref,index=refs.findIndex(candidate=>JSON.stringify(candidate)===JSON.stringify(ref));expect(index).toBeGreaterThanOrEqual(0);
   return {type:'content_chunk',chunk:chunk(ref,new TextEncoder().encode(text[index]))};
  });
  const ui=render(<JournalSourceProvider source={{client:server.client,target:server.target('A'),cut:()=>undefined,owned:()=>true}} content={bindings} messages={[messages[1]]} transcript={pages[1]} history={pages[0]}><section data-history><Message message={messages[0]}/></section><section data-live><Message message={messages[1]}/></section></JournalSourceProvider>);
  await waitFor(()=>expect(ui.container.querySelector('[data-history]')?.textContent).toBe(text[0]));expect(ui.container.querySelector('[data-live]')?.textContent).toBe(text[1]);
  expect(server.requests.filter(row=>row.request.method==='conversation/contentRead')).toHaveLength(2);
 }finally{cleanup();server.client.disconnect();}
});

it('a replaced child activation fences its held body even when the canonical reference is unchanged',async()=>{
 const {Server,checkpoint,snapshot}=await import('./fixture'),{JournalSourceProvider}=await import('../src/app/agent/ObservationSource'),{Message}=await import('../src/app/agent/Message');
 const server=new Server();await server.attached('A');server.held.add('conversation/contentRead');
 try {
  const text='Original body',bytes=new TextEncoder().encode(text),ref=reference('same-message',bytes.length),message={id:'same-message',role:'assistant' as const,content:[{type:'text' as const,text:'preview'}]};
  server.handlers.set('conversation/contentRead',()=>({type:'content_chunk',chunk:chunk(ref,bytes)}));
  const renderView=(activation_id:string)=><JournalSourceProvider source={{client:server.client,target:server.target('A'),agentId:'child',cut:()=>({...checkpoint(snapshot()).read_cut,activation_id}),owned:()=>true}} content={[binding(ref)]} messages={[message]}><Message message={message}/></JournalSourceProvider>;
  const ui=render(renderView('old'));const old=await server.waitFor('conversation/contentRead',1);ui.rerender(renderView('new'));const next=await server.waitFor('conversation/contentRead',2);
  await act(async()=>server.reply(old));expect(ui.queryByText(text)).toBeNull();await act(async()=>server.reply(next));await ui.findByText(text);
 }finally{cleanup();server.client.disconnect();}
});

it('streaming text uses its sparse native block index rather than its array position',async()=>{
 const {Server}=await import('./fixture'),{JournalSourceProvider}=await import('../src/app/agent/ObservationSource'),{Message}=await import('../src/app/agent/Message');
 const server=new Server();await server.attached('A');
 try {
  const text='Complete sparse streaming body',bytes=new TextEncoder().encode(text),base=reference('stream-message',bytes.length),ref:ContentRef={...base,source:{...base.source,block_index:7}};
  server.handlers.set('conversation/contentRead',request=>{if(request.method!=='conversation/contentRead')throw Error('method');expect(request.params.content_ref.source.block_index).toBe(7);return {type:'content_chunk',chunk:chunk(ref,bytes)};});
  const ui=render(<JournalSourceProvider source={{client:server.client,target:server.target('A'),cut:()=>undefined,owned:()=>true}} content={[binding(ref)]}><Message message={{id:'stream-message',role:'assistant',content:[]}} blocks={[{type:'text',block_index:7,text:'preview'}]} include={[7]} streaming/></JournalSourceProvider>);
  await ui.findByText(text);expect(ui.queryByText('preview')).toBeNull();
 }finally{cleanup();server.client.disconnect();}
});
