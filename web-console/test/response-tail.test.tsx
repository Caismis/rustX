import { act, cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { ConversationStats } from '../src/app/agent/ConversationStats';
import { prependTranscript, refreshTranscript, replaceTranscript } from '../src/client/transcript';
import type { CompletedResponseView, RuntimeClientSnapshot } from '../../protocol/app-server/v17';
import { snapshot } from './fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const response: CompletedResponseView = { closing_message_id: 'final-a', origin: { conversation_id: 'origin-conversation', closing_message_id: 'final-a', attempt_id: 'attempt-a' }, completed_at: '2026-09-19T08:00:00Z', surface_revision: '42', retry_message_id: 'input-a', usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 } };
function conversation(): RuntimeClientSnapshot {
  return { ...snapshot(), transcript: { entries: [
    { cursor: '1', item: { type: 'message', message: { role: 'user', id: 'input-a', source: 'human', timestamp: '2026-09-19T07:59:00Z', content: [{ type: 'text', text: 'Question' }] } } },
    { cursor: '2', item: { type: 'message', message: { role: 'assistant', id: 'internal-a', content: [{ type: 'text', text: 'Intermediate prose' }] } } },
    { cursor: '3', completed_response: response, item: { type: 'message', message: { role: 'assistant', id: 'final-a', content: [{ type: 'reasoning', text: 'Private reasoning' }, { type: 'text', text: 'Final ' }, { type: 'refusal', text: 'answer' }] } } },
  ], statistics: { completed_responses: '12', model_requests: '34', requests_with_usage: '34', reported_usage: { input_tokens: 1000, output_tokens: 200, total_tokens: 1200 } } } };
}
it('durable User chrome has Copy and only the native timestamp, without primary lineage actions', () => {
  const ui=render(<AgentTranscript snapshot={conversation()} onHistorical={vi.fn()}/>);
  const user=within(ui.getByLabelText('Your message'));
  expect(user.getByRole('button',{name:'Copy'})).toBeTruthy();
  expect(user.queryByRole('button',{name:/Fork|Branch|Retry|Lineage/})).toBeNull();
  expect(ui.getByLabelText('Your message').querySelector('time')?.dateTime).toBe('2026-09-19T07:59:00Z');
});
it('one completed response tail copies only final visible text and keeps missing facts absent', async () => {
  const writeText=vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText}});
  const ui=render(<AgentTranscript snapshot={conversation()} onHistorical={vi.fn()} lineageSwitchSafe/>);
  expect(ui.getAllByLabelText('Completed response')).toHaveLength(1);
  const tail=within(ui.getByLabelText('Completed response'));
  await act(async()=>fireEvent.click(tail.getByRole('button',{name:'Copy'})));
  expect(writeText).toHaveBeenCalledWith('Final answer');
  fireEvent.click(tail.getByRole('button',{name:'Usage 120 tokens'}));
  const detail=within(ui.getByRole('dialog',{name:'Usage'}));
  expect(detail.getByText('Input')).toBeTruthy();
  expect(detail.queryByText('Cache read')).toBeNull(); expect(detail.queryByText('Reasoning')).toBeNull();
  expect(ui.queryByText(/Ran for/)).toBeNull();
  expect(ui.container.textContent).not.toContain('attempt-a'); expect(ui.container.textContent).not.toContain('42');
});
it('unfinalized output has no fabricated tail, usage or lineage', () => {
  const state=conversation(); delete state.transcript.entries![2].completed_response;
  const ui=render(<AgentTranscript snapshot={state} onHistorical={vi.fn()}/>);
  expect(ui.queryByLabelText('Completed response')).toBeNull();
  expect(ui.queryByRole('button',{name:'Lineage'})).toBeNull();
});
it('lineage menu forwards the exact native response and keeps switching conservative', () => {
  const onHistorical=vi.fn(); const ui=render(<AgentTranscript snapshot={conversation()} onHistorical={onHistorical} lineageSwitchSafe={false}/>);
  fireEvent.click(ui.getByRole('button',{name:'Lineage'}));
  expect(ui.getByRole('button',{name:'Branch in this Session'})).toHaveProperty('disabled',true);
  expect(ui.getByRole('button',{name:'Retry / Regenerate'})).toHaveProperty('disabled',true);
  fireEvent.click(ui.getByRole('button',{name:'Fork to new Session'}));
  expect(onHistorical).toHaveBeenCalledWith('fork',response);
});
it('paging during live refresh preserves exact response facts and freshest native totals', () => {
  const state=conversation(); const first=replaceTranscript({...state.transcript,entries:[state.transcript.entries![2]],next_cursor:'3'});
  const latest={...state.transcript,statistics:{...state.transcript.statistics!,model_requests:'35'}};
  const live=refreshTranscript(first,latest);
  const merged=prependTranscript(live,{entries:state.transcript.entries!.slice(0,2),statistics:state.transcript.statistics});
  expect(merged.page.statistics?.model_requests).toBe('35');
  expect(merged.page.entries?.filter(entry=>entry.completed_response)).toHaveLength(1);
  expect(merged.page.entries?.at(-1)?.completed_response).toEqual(response);
});
it('composer totals ignore the loaded transcript and context uses only native measurement', () => {
  const state=conversation(); state.transcript.entries=[];
  state.context={compaction_count:0,compaction_in_progress:false,last_request_occupancy:{input_tokens:1024,context_window_tokens:4096,model:'historical-model'}};
  const ui=render(<ConversationStats snapshot={state}/>);
  expect(ui.getByText('12 responses · 34 requests')).toBeTruthy();
  expect(ui.getByRole('button',{name:'Conversation usage 1200 tokens'})).toBeTruthy();
  expect(ui.getByLabelText('Last request context 25%')).toBeTruthy();
  delete state.context!.last_request_occupancy;
  ui.rerender(<ConversationStats snapshot={state}/>);
  expect(ui.queryByLabelText('Last request context 25%')).toBeNull();
});
it('an unresolved response outside the fresh window is reread instead of freezing missing completion', () => {
  const state=conversation(); state.transcript.entries![2].response_pending=true;
  delete state.transcript.entries![2].completed_response;
  const old=replaceTranscript(state.transcript);
  const newer={entries:[state.transcript.entries![1],{cursor:'4',item:{type:'message' as const,message:{role:'user' as const,id:'new-user',source:'human' as const,content:[]}}}]};
  const refreshed=refreshTranscript(old,newer);
  expect(refreshed.epoch).toBe(old.epoch+1);
  expect(refreshed.error).toContain('reread unresolved native responses');
});

it('timing details use native whole runtime and distinguish first-request TTFT, missing values and zero', () => {
  const state = conversation();
  state.transcript.entries![2].completed_response = { ...response, timing: { total_duration_ms: 19000, ttft_ms: 320, generation_ms: 1280, output_tokens_per_second: 15.625 } };
  const ui = render(<AgentTranscript snapshot={state}/>);
  fireEvent.click(ui.getByRole('button', { name: 'Ran for 19 s' }));
  const detail = within(ui.getByRole('dialog', { name: 'Response timing' }));
  expect(detail.getByText('First request TTFT (from dispatch)')).toBeTruthy();
  expect(detail.getByText('0.32 s')).toBeTruthy();
  expect(detail.getByText('1.28 s')).toBeTruthy();
  expect(detail.getByText('15.6 tok/s')).toBeTruthy();
  fireEvent.click(ui.getByRole('button', { name: 'Close timing' }));
  state.transcript.entries![2].completed_response!.timing = { total_duration_ms: 0, generation_ms: 0 };
  ui.rerender(<AgentTranscript snapshot={state}/>);
  fireEvent.click(ui.getByRole('button', { name: 'Ran for 0 s' }));
  expect(ui.queryByText('First request TTFT (from dispatch)')).toBeNull();
  expect(ui.queryByText('Output speed')).toBeNull();
  expect(ui.getAllByText('0 s')).toHaveLength(2);
  fireEvent.click(ui.getByRole('button', { name: 'Close timing' }));
  state.transcript.entries![2].completed_response!.timing = { ttft_ms: 320 };
  ui.rerender(<AgentTranscript snapshot={state}/>);
  expect(ui.queryByRole('button', { name: /Ran for/ })).toBeNull();
});
