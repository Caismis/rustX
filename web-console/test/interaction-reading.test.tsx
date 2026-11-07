import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { CheckpointReadCut, InteractionKind, Request, RoutedInteraction, RuntimeClientInteractionHeader } from '../../protocol/app-server/v73';
import { Interactions } from '../src/app/agent/Interactions';
import { Server, checkpoint, interaction } from './fixture';

let server:Server|undefined;
afterEach(()=>{cleanup();server?.client.disconnect();});

it.each([false, true])('approval exposes Deny and Allow once without an extra cancellation action (bounded: %s)', async bounded => {
  const current = server = new Server(), original = interaction('approval');
  const header = bounded ? { ...original, body: null, body_complete: false, approval: { tool_id: 'tool-bash', tool_name: 'bash', reason: 'Developer approval required', mode: 'foreground' as const, deferred_fields: [['arguments']] } } : original;
  current.snapshots.get('A')!.pending_interactions = [header];
  if (bounded) current.held.add('conversation/checkpointValueRead');
  await current.attached('A');
  function Pending() { const state = useSyncExternalStore(current.client.subscribe, current.client.getSnapshot); return <Interactions client={current.client} state={state} view={state.views.A}/>; }
  render(<Pending/>);
  expect(screen.queryByRole('button', { name: 'Cancel interaction' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Allow once' })).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Deny' })));
  const request = await current.waitForReply();
  expect('result' in request ? request.result : undefined).toMatchObject( { type: 'approval', decision: { type: 'deny' } });
  expect(current.replies.some(reply=>'error' in reply)).toBe(false);
});

const approvalTools: Pick<Extract<InteractionKind, { type: 'approval' }>, 'tool_id' | 'tool_name' | 'origin'>[] = [
  { tool_id: 'tool-bash', tool_name: 'shell', origin: 'builtin' },
  { tool_id: 'mcp-cleanup', tool_name: 'bash', origin: { mcp: { server_id: 'cleanup-server' } } },
  { tool_id: 'managed-cleanup', tool_name: 'cleanup', origin: { managed_python: { package: 'cleanup' } } },
];
for (const bounded of [false, true]) it.each(approvalTools)('approval renders the exact $tool_id arguments before consent (bounded: '+bounded+')', async tool => {
  const current = server = new Server(), original = interaction('approval');
  if (!original.body || original.body.kind.type !== 'approval') throw new Error('Expected approval fixture.');
  const args = { command: 'delete', target: '/workspace/待删除', recursive: true };
  const kind = { ...original.body.kind, ...tool, arguments: args };
  const header: RuntimeClientInteractionHeader = bounded
    ? { ...original, body: null, body_complete: false, approval: { tool_id: tool.tool_id, tool_name: tool.tool_name, reason: kind.reason, mode: kind.mode, deferred_fields: [['arguments']] } }
    : { ...original, body: { ...original.body, kind } };
  current.snapshots.get('A')!.pending_interactions = [header];
  const bytes = new TextEncoder().encode(JSON.stringify(args));
  // Split within a multibyte target character to exercise the deferred decoder.
  const split = bytes.findIndex(byte => byte > 127) + 1;
  current.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw new Error('Expected arguments read.');
    expect(request.params.path).toEqual(['request', 'kind', 'arguments']);
    const offset = request.params.offset, end = offset === 0 ? split : bytes.length;
    return { type: 'checkpoint_value_chunk', chunk: { cut: request.params.cut, domain: 'pending_interactions', item_offset: 0, path: request.params.path,
      offset, readable_end: bytes.length, bytes_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))), next_offset: end < bytes.length ? end : null, representation: 'canonical_json_v1' } };
  });
  await current.attached('A');
  function Pending() { const state = useSyncExternalStore(current.client.subscribe, current.client.getSnapshot); return <Interactions client={current.client} state={state} view={state.views.A}/>; }
  render(<Pending/>);
  const details = screen.getByRole('group', { name: 'Approval details' });
  await waitFor(() => expect(details.querySelector('pre')).toBeTruthy());
  const text = details.querySelector('pre')!.textContent;
  if (tool.tool_id === 'tool-bash') expect(text).toBe(args.command);
  else expect(JSON.parse(text!)).toEqual(args);
  expect(current.replies.length > 0).toBe(false);
  if (bounded) expect(current.requests.filter(row => row.request.method === 'conversation/checkpointValueRead')).toHaveLength(2);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Allow once' })));
  const request = await current.waitForReply();
  expect('result' in request ? request.result : undefined).toMatchObject( { type: 'approval', decision: { type: 'allow' } });
});

it('a deferred approval with no canonical tool identity preserves all arguments', async () => {
  const current = server = new Server(), original = interaction('approval');
  current.snapshots.get('A')!.pending_interactions = [{ ...original, body: null, body_complete: false,
    approval: { tool_name: 'bash', reason: 'Developer approval required', mode: 'foreground', deferred_fields: [['arguments']] } }];
  const args = { command: 'delete', target: '/workspace/data', recursive: true }, text = JSON.stringify(args);
  current.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw new Error('Expected arguments read.');
    return { type: 'checkpoint_value_chunk', chunk: { cut: request.params.cut, domain: 'pending_interactions', item_offset: 0, path: request.params.path, offset: 0,
      readable_end: text.length, bytes_base64: btoa(text), next_offset: null, representation: 'canonical_json_v1' } };
  });
  await current.attached('A');
  function Pending() { const state = useSyncExternalStore(current.client.subscribe, current.client.getSnapshot); return <Interactions client={current.client} state={state} view={state.views.A}/>; }
  render(<Pending/>);
  await waitFor(() => expect(screen.getByRole('group', { name: 'Approval details' }).querySelector('pre')).toBeTruthy());
  expect(JSON.parse(screen.getByRole('group', { name: 'Approval details' }).querySelector('pre')!.textContent!)).toEqual(args);
});

it('keeps the current approval command visible when the pending collection refreshes', async () => {
  const current = server = new Server();
  const header: RuntimeClientInteractionHeader = { ...interaction('approval'), body: null, body_complete: false,
    approval: { tool_id: 'tool-bash', tool_name: 'bash', reason: 'Developer approval required', mode: 'foreground', deferred_fields: [['arguments']] } };
  current.snapshots.get('A')!.pending_interactions = [header];
  current.handlers.set('session/checkpoint', () => ({ type: 'checkpoint', checkpoint: { ...checkpoint(current.snapshots.get('A')!), read_cut: { ...checkpoint(current.snapshots.get('A')!).read_cut, domain_revisions: { pending_interactions: '1' } } } }));
  current.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw Error('Expected arguments read');
    const text = JSON.stringify({ command: 'printf rustX' });
    return { type: 'checkpoint_value_chunk', chunk: { cut: request.params.cut, domain: 'pending_interactions', item_offset: 0, path: request.params.path, offset: 0, readable_end: text.length, bytes_base64: btoa(text), next_offset: null, representation: 'canonical_json_v1' } };
  });
  await current.attached('A');
  function Pending() { const state = useSyncExternalStore(current.client.subscribe, current.client.getSnapshot); return <Interactions client={current.client} state={state} view={state.views.A}/>; }
  render(<Pending/>);
  const command = await screen.findByText('printf rustX');
  current.held.add('conversation/checkpointValueRead');
  const next = checkpoint(current.snapshots.get('A')!, '1'); next.read_cut.domain_revisions.pending_interactions = '2';
  await act(async () => current.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: current.target('A'), cursor: '1', event: { type: 'checkpoint_advanced', transition: 'pending-refresh', checkpoint: next } } }));
  const request = await current.waitFor('conversation/checkpointValueRead', 2);
  expect(screen.getByText('printf rustX')).toBe(command);
  expect(screen.queryByText(/Loading/)).toBeNull();
  await act(async () => current.reply(request));
  expect(screen.getByText('printf rustX')).toBe(command);
});

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
    for(const header of current.snapshots.get('A')!.pending_interactions??[]) current.socket.deliver({jsonrpc:'2.0',id:`callback:${header.interaction.interaction_id}`,method:'item/interaction/request',params:{threadId:current.target('A').conversation_id,interaction:header}});
  };
  return {current,publish};
}

it('pending details have no generic cancel/kind controls and recover through a scoped read retry', async () => {
  const { current } = await pendingFixture();
  expect(screen.getByRole('status').textContent).toBe('Loading…');
  expect(screen.queryByRole('button', { name: 'Cancel interaction' })).toBeNull();
  const page = await current.waitFor('conversation/checkpointPage', 1);
  await act(async () => current.socket.deliver({ jsonrpc: '2.0', id: page.id, error: { code: -32000, message: 'Details temporarily unavailable' } }));
  expect(screen.getByRole('alert').textContent).toContain('Details temporarily unavailable');
  expect(screen.queryByRole('status')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Retry loading details' }));
  const retry = await current.waitFor('conversation/checkpointPage', 2);
  await act(async () => current.reply(retry));
  const value = await current.waitFor('conversation/checkpointValueRead', 1);
  await act(async () => current.reply(value));
  expect(await screen.findByRole('region', { name: 'Questionnaire' })).toBeTruthy();
  expect(current.replies.length > 0).toBe(false);
});

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
  expect(current.replies).toHaveLength(0);
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
  expect(current.replies).toHaveLength(0);
});
