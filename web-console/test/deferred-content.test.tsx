import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { DeferredContent } from '../src/app/agent/DeferredContent';
import type { ContentBinding, ContentChunk, ContentRef } from '../../protocol/app-server/v51';

afterEach(cleanup);
const reference=(message:string,bytes:number):ContentRef=>({conversation_id:'conversation-A',source:{type:'live_publication_prefix',component:'payload',stream_id:'stream-A',message_id:message,attempt_id:'attempt-A',turn_id:'turn-A',publication_request_id:'request-A',block_index:0},representation:'utf8_text',version:1,content_revision:1,readable_end:bytes});
const binding=(ref:ContentRef):ContentBinding=>({path:['messages','0','content','0','text'],content_ref:ref,deferred:true});
const chunk=(ref:ContentRef,bytes:Uint8Array):ContentChunk=>({content_ref:ref,offset:0,end_offset:bytes.length,readable_end:bytes.length,data_base64:btoa(String.fromCharCode(...bytes)),next_offset:null,complete:true});

it('T11 replaces content identity before a gated old read completes', async()=>{
  const old=reference('old',3),next=reference('next',3);
  let release!:(value:ContentChunk)=>void;
  let oldOwner!:()=>boolean;
  const read=(_ref:ContentRef,_offset:number,current:()=>boolean)=>{oldOwner=current;return new Promise<ContentChunk>(resolve=>{release=resolve;});};
  const owned=()=>true;
  const ui=render(<DeferredContent bindings={[binding(old)]} owned={owned} read={read}/>);
  fireEvent.click(ui.getAllByRole('button')[0]!);
  ui.rerender(<DeferredContent bindings={[binding(next)]} owned={owned} read={read}/>);
  expect(oldOwner()).toBe(false);
  await act(async()=>{release(chunk(old,new TextEncoder().encode('old')));});
  expect(ui.queryByText('old')).toBeNull();
  expect(ui.getAllByRole('button')[0]!.hasAttribute('disabled')).toBe(false);
});

it('T08 displays BinaryBytes as exact Base64 rather than UTF-8',async()=>{
  const bytes=Uint8Array.of(0,255,128,10,65,0);
  const ref={...reference('binary',bytes.length),representation:'binary_bytes' as const};
  const ui=render(<DeferredContent bindings={[binding(ref)]} owned={()=>true} read={async()=>chunk(ref,bytes)}/>);
  await act(async()=>{fireEvent.click(ui.getAllByRole('button')[0]!);});
  expect(ui.getByText(btoa(String.fromCharCode(...bytes)))).toBeTruthy();
});
it('an incidental ownership callback replacement preserves a held same-source read and its loaded window',async()=>{
 const ref=reference('same-source',3);let finish!:(value:ContentChunk)=>void,owns!:()=>boolean;
 const read=(_ref:ContentRef,_offset:number,current:()=>boolean)=>{owns=current;return new Promise<ContentChunk>(resolve=>{finish=resolve;});};
 const ui=render(<DeferredContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);fireEvent.click(ui.getAllByRole('button')[0]!);
 ui.rerender(<DeferredContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);expect(owns()).toBe(true);await act(async()=>finish(chunk(ref,new TextEncoder().encode('old'))));expect(ui.getByText('old')).toBeTruthy();
 ui.rerender(<DeferredContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);expect(ui.getByText('old')).toBeTruthy();
});
it('a replacement read owner or false current ownership rejects a held same-reference reply',async()=>{
 for(const replaceReader of [false,true]){
  const ref=reference(`retired-${replaceReader}`,3);let finish!:(value:ContentChunk)=>void,owns!:()=>boolean;
  const read=(_ref:ContentRef,_offset:number,current:()=>boolean)=>{owns=current;return new Promise<ContentChunk>(resolve=>{finish=resolve;});};
  const ui=render(<DeferredContent bindings={[binding(ref)]} owned={()=>true} read={read}/>);fireEvent.click(ui.getAllByRole('button')[0]!);
  ui.rerender(<DeferredContent bindings={[binding(ref)]} owned={()=>replaceReader} read={replaceReader?async()=>chunk(ref,new TextEncoder().encode('new')):read}/>);expect(owns()).toBe(false);await act(async()=>finish(chunk(ref,new TextEncoder().encode('old'))));expect(ui.queryByText('old')).toBeNull();ui.unmount();
 }
});
