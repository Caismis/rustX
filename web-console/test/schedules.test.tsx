import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ScheduleHeader, ScheduledWorkflowRecovery } from '../../protocol/app-server/v57';
import { Schedules } from '../src/app/agent/Schedules';
import { Server } from './fixture';

afterEach(cleanup);
const nodeA = 'node_00000000-0000-7000-8000-000000000001';
const nodeB = 'node_00000000-0000-7000-8000-000000000002';
function row(title = 'Native reminder'): ScheduleHeader {
  return { id:{conversation_id:'conversation-A',ordinal:'9007199254740993'}, binding:{session_id:'A',node_id:nodeA,conversation_id:'conversation-A'},
    revision:'9007199254740993',title,target:{type:'conversation_inbound'},spec:{type:'at',at_utc:'2026-10-24T10:00:00Z'},state:'enabled',next_due_utc:'2026-10-24T10:00:00Z',
    last_trigger:null,completed_at:null,completion_reason:null,last_failure:null };
}

it('reads a cold exact-node header page and loads reminder text only on request', async () => {
  const server = new Server();
  server.handlers.set('schedule/list', () => ({type:'schedules',schedules:[row()],next_ordinal:null}));
  server.handlers.set('schedule/get', () => ({type:'schedule',schedule:{...row(),target:{type:'conversation_inbound',text:'Full native reminder text'}}}));
  await server.connect();
  render(<Schedules client={server.client} sessionId="A" nodeId={nodeA}/>);
  await act(async () => fireEvent.click(screen.getByRole('button',{name:'Schedules'})));
  expect(await screen.findByText('Native reminder')).toBeTruthy();
  const list = await server.waitFor('schedule/list',1);
  if (list.method !== 'schedule/list') throw new Error('Expected native list');
  expect(list.params).toEqual({session_id:'A',node_id:nodeA,after_ordinal:'0',limit:20});
  expect(server.requests.some(item => item.request.method === 'schedule/get')).toBe(false);
  await act(async () => fireEvent.click(screen.getByRole('button',{name:'Read message'})));
  expect(await screen.findByText('Full native reminder text')).toBeTruthy();
  const detail = await server.waitFor('schedule/get',1);
  if (detail.method !== 'schedule/get') throw new Error('Expected native get');
  expect(detail.params.id.ordinal).toBe('9007199254740993');
  expect(server.claims()).toEqual([]);
  expect(server.coldLoads.size).toBe(0);
});

it('keeps the opened native node when focus changes and ignores an obsolete page', async () => {
  const server = new Server();
  server.held.add('schedule/list');
  server.handlers.set('schedule/list', request => {
    if (request.method !== 'schedule/list') throw new Error('Expected native list');
    return {type:'schedules',schedules:[row(request.params.node_id === nodeA ? 'Old node reminder' : 'New node reminder')],next_ordinal:null};
  });
  await server.connect();
  const ui = render(<Schedules client={server.client} sessionId="A" nodeId={nodeA}/>);
  await act(async () => fireEvent.click(screen.getByRole('button',{name:'Schedules'})));
  const old = await server.waitFor('schedule/list',1);
  ui.rerender(<Schedules client={server.client} sessionId="A" nodeId={nodeB}/>);
  expect(screen.getByText(`Session A · Node ${nodeA}`)).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button',{name:'Close'})));
  await act(async () => fireEvent.click(screen.getByRole('button',{name:'Schedules'})));
  const fresh = await server.waitFor('schedule/list',2);
  if (fresh.method !== 'schedule/list') throw new Error('Expected new native list');
  expect(fresh.params.node_id).toBe(nodeB);
  await act(async () => { server.reply(fresh); });
  expect(await screen.findByText('New node reminder')).toBeTruthy();
  await act(async () => { server.reply(old); });
  expect(screen.queryByText('Old node reminder')).toBeNull();
  expect(server.claims()).toEqual([]);
});

function recoveryFixture(): {header:ScheduleHeader;recovery:ScheduledWorkflowRecovery} {
  const header=row('Native workflow');header.target={type:'workflow',workflow_id:'review'};
  const startId='swf_'+ 'a'.repeat(64), time='2026-10-24T10:00:00Z';
  const run={conversation_id:header.id.conversation_id,origin:{type:'scheduled' as const,start_id:startId},invocation:'1'};
  const trigger={schedule_id:header.id,admission_revision:header.revision,occurrence_utc:time};
  header.last_trigger={trigger_id:trigger,accepted_at:time,target:{type:'workflow',start_id:startId,run_id:run}};
  return {header,recovery:{start:{id:startId,run_id:run,trigger_id:trigger,binding:header.binding,accepted_at:time,state:{type:'claimed',owner:'native-owner',claimed_at:time,effects_started:true},cancellation_requested_at:time,recovery_blocked:'Original Bash supervisor has no positive physical proof',native_quiescence_proven:false},effects:[{ordinal:'9007199254740993',node:{block:{run,definition:{workflow_id:'review',blocks:[]},invocations:[]},node:'work',visit:0},kind:'tool',owner:{type:'preparing'},physical_settlement_proven:false}],capacity_blocked:true,deletion_blocked:true,next_effect_ordinal:'9007199254740993'}};
}
it('reads blocked Workflow ownership and a bounded continuation without activating or mutating',async()=>{
  const server=new Server(), fixture=recoveryFixture();
  server.handlers.set('schedule/list',()=>({type:'schedules',schedules:[fixture.header],next_ordinal:null}));
  server.handlers.set('workflow/scheduled/getRecovery',()=>({type:'scheduled_workflow_recovery',recovery:fixture.recovery}));
  await server.connect();const baseline=server.requests.length;render(<Schedules client={server.client} sessionId="A" nodeId={nodeA}/>);
  await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Schedules'})));
  expect(await screen.findByText('Native workflow')).toBeTruthy();
  await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Read execution and recovery'})));
  expect(await screen.findByText('Original Bash supervisor has no positive physical proof')).toBeTruthy();
  expect(screen.getByText('This run still holds a scheduling slot and prevents deleting this Session.')).toBeTruthy();
  await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Next effects'})));
  const request=await server.waitFor('workflow/scheduled/getRecovery',2);
  if(request.method!=='workflow/scheduled/getRecovery')throw new Error('Native recovery read');
  expect(request.params.after_effect_ordinal).toBe('9007199254740993');
  expect(server.requests.slice(baseline).every(item=>['schedule/list','workflow/scheduled/getRecovery'].includes(item.request.method))).toBe(true);
  expect(server.claims()).toEqual([]);expect(server.coldLoads.size).toBe(0);
});
it('discards recovery bodies after closing the exact-node inspection',async()=>{
  const server=new Server(),fixture=recoveryFixture();server.held.add('workflow/scheduled/getRecovery');
  server.handlers.set('schedule/list',()=>({type:'schedules',schedules:[fixture.header],next_ordinal:null}));
  server.handlers.set('workflow/scheduled/getRecovery',()=>({type:'scheduled_workflow_recovery',recovery:fixture.recovery}));
  await server.connect();render(<Schedules client={server.client} sessionId="A" nodeId={nodeA}/>);
  await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Schedules'})));
  await screen.findByText('Native workflow');await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Read execution and recovery'})));
  const held=await server.waitFor('workflow/scheduled/getRecovery',1);
  await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Close'})));
  await act(async()=>{server.reply(held);});
  expect(screen.queryByText('Original Bash supervisor has no positive physical proof')).toBeNull();
});
