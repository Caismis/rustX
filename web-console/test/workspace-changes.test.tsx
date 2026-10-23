import { afterEach,expect,it,vi } from 'vitest';
import { act,cleanup,render,fireEvent,waitFor } from '@testing-library/react';
import { ChangeComparison } from '../src/app/components/ChangesReview';
import { ChangesReviewContext } from '../src/app/changes-review';
import { TurnChanges } from '../src/app/agent/TurnChanges';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import { conversationPreferences } from '../src/app/conversation-preferences';
import { Server,snapshot,checkpoint } from './fixture';
import type { ChangesSummary,RuntimeClientSnapshot } from '../../protocol/app-server/v54';

afterEach(()=>{cleanup();conversationPreferences().update({codingView:true},false);});
const summary:ChangesSummary={files:Array.from({length:6},(_,i)=>({path:`/workspace/A/file-${i}.ts`,display:`file-${i}.ts`,added:2,deleted:1,binary:false,oversized:false})),total:6,added:12,deleted:6};
async function sourceFixture() {
  const server=new Server();await server.attached('A');
  const source={client:server.client,target:server.client.target('A')!,owned:()=>true,cut:()=>checkpoint(snapshot()).read_cut};
  server.handlers.set('conversation/changes',()=>({type:'workspace_changes',summary}));
  return {server,source};
}

it('a late comparison never appears under another file after a rapid switch',async()=>{
  const {server,source}=await sourceFixture();
  try {
    server.held.add('conversation/changeDiff');
    const request={client:server.client,target:source.target,attemptId:'attempt-1',summary,index:0,owned:source.owned};
    const ui=render(<ChangeComparison request={request} index={0}/>);
    const first=await server.waitFor('conversation/changeDiff',1);
    ui.rerender(<ChangeComparison request={request} index={1}/>);
    const second=await server.waitFor('conversation/changeDiff',2);
    const comparison=(text:string)=>({type:'workspace_change_diff' as const,comparison:{hunks:[{old_start:1,new_start:1,old_text:'old\n',new_text:text}],binary:false,oversized:false,truncated:false}});
    await act(async()=>server.socket.success(second,comparison('second file\n')));
    expect(ui.getByText('second file')).toBeTruthy();
    await act(async()=>server.socket.success(first,comparison('obsolete file\n')));
    expect(ui.queryByText('obsolete file')).toBeNull();
    expect(ui.getByText('file-1.ts:1')).toBeTruthy();
  } finally {server.client.disconnect();}
});

it('replaced ownership restarts a pending comparison even for the same file identity',async()=>{
  const {server,source}=await sourceFixture();
  let firstCurrent=true;
  try {
    server.held.add('conversation/changeDiff');
    const request={client:server.client,target:source.target,attemptId:'attempt-1',summary,index:0,owned:()=>firstCurrent};
    const ui=render(<ChangeComparison request={request} index={0}/>);
    const first=await server.waitFor('conversation/changeDiff',1);
    firstCurrent=false;
    ui.rerender(<ChangeComparison request={{...request,owned:()=>true}} index={0}/>);
    const second=await server.waitFor('conversation/changeDiff',2);
    const comparison=(text:string)=>({type:'workspace_change_diff' as const,comparison:{hunks:[{old_start:1,new_start:1,old_text:'old\n',new_text:text}],binary:false,oversized:false,truncated:false}});
    await act(async()=>server.socket.success(second,comparison('current owner\n')));
    await act(async()=>server.socket.success(first,comparison('retired owner\n')));
    expect(ui.getByText('current owner')).toBeTruthy();
    expect(ui.queryByText('retired owner')).toBeNull();
  } finally {server.client.disconnect();}
});

it('replacing the chat owner restarts an unresolved summary under the same attachment',async()=>{
  const {server,source}=await sourceFixture();
  let firstCurrent=true;
  try {
    server.held.add('conversation/changes');
    const firstSource={...source,owned:()=>firstCurrent};
    const ui=render(<JournalSourceProvider source={firstSource}><TurnChanges attemptId="attempt-1" conversationId="conversation-A"/></JournalSourceProvider>);
    const first=await server.waitFor('conversation/changes',1);
    firstCurrent=false;
    ui.rerender(<JournalSourceProvider source={{...source,owned:()=>true}}><TurnChanges attemptId="attempt-1" conversationId="conversation-A"/></JournalSourceProvider>);
    const second=await server.waitFor('conversation/changes',2);
    await act(async()=>server.socket.success(second,{type:'workspace_changes',summary}));
    await act(async()=>server.socket.success(first,{type:'workspace_changes',summary:null}));
    expect(ui.getByText('Edited 6 files')).toBeTruthy();
  } finally {server.client.disconnect();}
});

it('renders native counts, folds at four files, opens the exact file and reads no comparison before hover',async()=>{
  const {server,source}=await sourceFixture(),open=vi.fn();
  try {
    const ui=render(<JournalSourceProvider source={source}><ChangesReviewContext value={open}><TurnChanges attemptId="attempt-1" conversationId="conversation-A"/></ChangesReviewContext></JournalSourceProvider>);
    await ui.findByText('Edited 6 files');
    expect(ui.getAllByRole('listitem')).toHaveLength(4);
    fireEvent.click(ui.getByRole('button',{name:'All 6 files'}));
    expect(ui.getAllByRole('listitem')).toHaveLength(6);
    fireEvent.click(ui.getByRole('button',{name:'View changes to file-5.ts'}));
    expect(open).toHaveBeenCalledWith(expect.objectContaining({attemptId:'attempt-1',index:5,target:source.target,summary}));
    expect(server.requests.some(row=>row.request.method==='conversation/changeDiff')).toBe(false);
  } finally {server.client.disconnect();}
});

it('sidebar comparisons outlive a retired chat view but stop with the original attachment',async()=>{
  const {server,source}=await sourceFixture(),open=vi.fn();
  let chatCurrent=true;
  source.owned=()=>chatCurrent;
  server.handlers.set('conversation/changeDiff',()=>({type:'workspace_change_diff',comparison:{hunks:[{old_start:1,new_start:1,old_text:'old\n',new_text:'still available\n'}],binary:false,oversized:false,truncated:false}}));
  try {
    const chat=render(<JournalSourceProvider source={source}><ChangesReviewContext value={open}><TurnChanges attemptId="attempt-1" conversationId="conversation-A"/></ChangesReviewContext></JournalSourceProvider>);
    await chat.findByText('Edited 6 files');
    fireEvent.click(chat.getByRole('button',{name:'View changes to file-0.ts'}));
    const request=open.mock.calls[0][0];
    chatCurrent=false;chat.unmount();
    const sidebar=render(<ChangeComparison request={request} index={0}/>);
    await sidebar.findByText('still available');
    await act(async()=>server.client.release('A'));
    expect(sidebar.getByText('Changes from this turn are no longer available.')).toBeTruthy();
  } finally {server.client.disconnect();}
});

it('places an actual changes card after the final reply and before completed-turn actions',async()=>{
  const {server,source}=await sourceFixture();
  const state:RuntimeClientSnapshot=snapshot();
  const message={role:'assistant' as const,id:'final',content:[{type:'text' as const,text:'The implementation is complete.'}]};
  state.messages=[message];state.transcript.entries=[{cursor:'1',item:{type:'message',message},completed_response:{completed_at:'2026-10-22T05:00:00Z',closing_message_id:'final',closing_locator:{type:'by_message_id',message_id:'final'},deferred_fields:[],surface_revision:'1',origin:{conversation_id:'conversation-A',attempt_id:'attempt-1',attempt_id_source:null,closing_message_id:'final'}}}];
  try {
    const ui=render(<JournalSourceProvider source={source}><AgentTranscript snapshot={state}/></JournalSourceProvider>);
    const title=await ui.findByText('Edited 6 files'),card=title.closest('[data-changed-files]')!;
    expect(ui.getByText('The implementation is complete.').compareDocumentPosition(card)&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(card.compareDocumentPosition(ui.getByLabelText('Completed Turn'))&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(ui.container.querySelectorAll('[data-changed-files]')).toHaveLength(1);
  } finally {server.client.disconnect();}
});

it('absent records have no placeholder card and another conversation never borrows current changes',async()=>{
  const {server,source}=await sourceFixture();
  try {
    server.handlers.set('conversation/changes',()=>({type:'workspace_changes',summary:null}));
    const ui=render(<JournalSourceProvider source={source}><TurnChanges attemptId="gone" conversationId="conversation-A"/></JournalSourceProvider>);
    await server.waitFor('conversation/changes',1);await waitFor(()=>expect(ui.container.querySelector('[data-changed-files]')).toBeNull());
    const count=server.requests.length;
    ui.rerender(<JournalSourceProvider source={source}><TurnChanges attemptId="old" conversationId="old-conversation"/></JournalSourceProvider>);
    await waitFor(()=>expect(server.requests.length).toBe(count));
  } finally {server.client.disconnect();}
});

it('coding view and child scopes do not request or show top-level change records',async()=>{
  const {server,source}=await sourceFixture();
  try {
    conversationPreferences().update({codingView:false},false);
    const ui=render(<JournalSourceProvider source={source}><TurnChanges attemptId="attempt-1" conversationId="conversation-A"/></JournalSourceProvider>);
    expect(ui.container.querySelector('[data-changed-files]')).toBeNull();
    expect(server.requests.some(row=>row.request.method==='conversation/changes')).toBe(false);
    ui.unmount();conversationPreferences().update({codingView:true},false);
    render(<JournalSourceProvider source={{...source,agentId:'child'}}><TurnChanges attemptId="attempt-1" conversationId="conversation-A"/></JournalSourceProvider>);
    expect(server.requests.some(row=>row.request.method==='conversation/changes')).toBe(false);
  } finally {server.client.disconnect();}
});
