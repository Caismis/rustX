import { useSyncExternalStore } from 'react';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { CheckpointReadCut, Request, RoutedInteraction, RuntimeClientInteractionHeader } from '../../protocol/app-server/v57';
import { Interactions } from '../src/app/agent/Interactions';
import { Server, checkpoint, interaction } from './fixture';

let server:Server|undefined;
afterEach(()=>{cleanup();server?.client.disconnect();});

async function pendingFixture(){
  const current=server=new Server();
  let header:RuntimeClientInteractionHeader=interaction('questionnaire');
  let body:RoutedInteraction={interaction:header.interaction,source:header.source,request:header.body!};
  header={...header,body:null,body_complete:false};
  let revision='0',pendingRevision='1',generation='1';
  const cut=():CheckpointReadCut=>({...checkpoint(current.snapshots.get('A')!,revision).read_cut,
    domain_revisions:{pending_interactions:pendingRevision},collection_cuts:{pending_primary:{generation,total:1},pending_children:{generation:'0',total:0}}});
  current.snapshots.get('A')!.pending_interactions=[header];
  current.handlers.set('session/checkpoint',()=>({type:'checkpoint',checkpoint:{...checkpoint(current.snapshots.get('A')!,revision),read_cut:cut()}}));
  current.handlers.set('conversation/checkpointPage',request=>{
    if(request.method!=='conversation/checkpointPage')throw new Error('Expected pending Page.');
    expect(request.params.domain).toBe('pending_interactions');
    expect(request.params.offset).toBe(header.source_offset);
    return {type:'checkpoint_page',page:{cursor:request.params.cut.revision,read_cut:request.params.cut,domain:request.params.domain,total:'1',items:[{offset:String(header.source_offset),identity:header.request_id,value:header,content:[],deferred:false,body_complete:false}],next_offset:null}};
  });
  current.handlers.set('conversation/checkpointValueRead',request=>{
    if(request.method!=='conversation/checkpointValueRead')throw new Error('Expected pending ValueRead.');
    expect(request.params.domain).toBe('pending_interactions');
    expect(request.params.path).toEqual([]);
    const bytes=new TextEncoder().encode(JSON.stringify(body));
    return {type:'checkpoint_value_chunk',chunk:{cut:request.params.cut,domain:request.params.domain,item_offset:request.params.item_offset,path:request.params.path,offset:0,readable_end:bytes.length,bytes_base64:btoa(String.fromCharCode(...bytes)),next_offset:null,representation:'canonical_json_v1'}};
  });
  await current.attached('A');
  current.held.add('conversation/checkpointPage');current.held.add('conversation/checkpointValueRead');
  function Pending(){const state=useSyncExternalStore(current.client.subscribe,current.client.getSnapshot);return <Interactions client={current.client} state={state} view={state.views.A}/>;}
  render(<Pending/>);
  const publish=async(replace=false)=>{
    if(replace){
      const replacement=interaction('questionnaire','A','replacement-question');
      body={interaction:replacement.interaction,source:replacement.source,request:replacement.body!};
      if(body.request.kind.type==='questionnaire')body.request.kind.questionnaire.questions[0]!.question='Original replacement question';
      header={...replacement,body:null,body_complete:false};pendingRevision='2';generation='2';
    }
    revision=String(BigInt(revision)+1n);current.cursor=BigInt(revision);
    current.snapshots.get('A')!.pending_interactions=[header];
    await act(async()=>current.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:current.target('A'),cursor:revision,event:{type:'checkpoint_advanced',transition:'pending-source-owner',checkpoint:{...checkpoint(current.snapshots.get('A')!,revision),read_cut:cut()}}}}));
  };
  return {current,publish};
}

it('an unrelated checkpoint preserves the selected pending read and its original Page/Value cut',async()=>{
  const {current,publish}=await pendingFixture();
  const page=await current.waitFor('conversation/checkpointPage',1);
  if(page.method!=='conversation/checkpointPage')throw new Error('Expected Page.');
  const originalCut=structuredClone(page.params.cut);
  await publish();
  expect(current.client.getSnapshot().views.A.checkpoint!.read_cut.revision).toBe('1');
  await act(async()=>current.reply(page));
  const value=await current.waitFor('conversation/checkpointValueRead',1);
  if(value.method!=='conversation/checkpointValueRead')throw new Error('Expected ValueRead.');
  expect(value.params.cut).toEqual(originalCut);
  expect(value.params.cut.revision).toBe('0');
  await act(async()=>current.reply(value));
  await waitFor(()=>expect(screen.getByRole('region',{name:'Questionnaire'})).toBeTruthy());
  expect(screen.getByText('Which direction?')).toBeTruthy();
  expect(current.requests.filter(row=>row.request.method==='interaction/respond')).toHaveLength(0);
});

it.each(['page','value'] as const)('a replaced pending owner resets loading while its held %s cannot apply late',async(held)=>{
  const {current,publish}=await pendingFixture();
  const page=await current.waitFor('conversation/checkpointPage',1);
  let retired:Request=page;
  if(held==='value'){
    await act(async()=>current.reply(page));
    retired=await current.waitFor('conversation/checkpointValueRead',1);
  }
  // This is a genuine successful old-source response, computed before the new
  // native owner and held only at its carrier delivery boundary.
  const late=current.commit(retired);
  await publish(true);
  await current.waitFor('conversation/checkpointPage',2);
  await act(async()=>current.socket.deliver(late));
  expect(screen.queryByText('Which direction?')).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
  const replacement=await current.waitFor('conversation/checkpointPage',2);
  if(replacement.method!=='conversation/checkpointPage')throw new Error('Expected replacement Page.');
  expect(replacement.params.cut.domain_revisions.pending_interactions).toBe('2');
  await act(async()=>current.reply(replacement));
  const value=await current.waitFor('conversation/checkpointValueRead',held==='value'?2:1);
  await act(async()=>current.reply(value));
  await waitFor(()=>expect(screen.getByText('Original replacement question')).toBeTruthy());
  expect(screen.queryByText('Which direction?')).toBeNull();
  expect(current.requests.filter(row=>row.request.method==='interaction/respond')).toHaveLength(0);
});
