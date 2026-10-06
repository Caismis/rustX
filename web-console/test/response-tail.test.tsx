import { act, cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { ConversationStats } from '../src/app/agent/UsageStats';
import { localeController } from '../src/locale/controller';
import { prependTranscript, refreshTranscript, replaceTranscript } from '../src/client/transcript';
import type { CompletedResponseView, RuntimeClientSnapshot } from '../../protocol/app-server/v36';
import { snapshot } from './fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); act(() => localeController.setLocale('en')); });
const response: CompletedResponseView = { closing_message_id: 'final-a', origin: { conversation_id: 'origin-conversation', closing_message_id: 'final-a', attempt_id: 'attempt-a' }, completed_at: '2026-09-19T08:00:00Z', surface_revision: '42', retry_message_id: 'input-a', usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 } };
function conversation(): RuntimeClientSnapshot {
  return { ...snapshot(), transcript: { entries: [
    { cursor: '1', item: { type: 'message', message: { role: 'user', id: 'input-a', source: 'human', timestamp: '2026-09-19T07:59:00Z', content: [{ type: 'text', text: 'Question' }] } } },
    { cursor: '2', item: { type: 'message', message: { role: 'assistant', id: 'internal-a', content: [{ type: 'text', text: 'Intermediate prose' }] } } },
    { cursor: '3', completed_response: response, item: { type: 'message', message: { role: 'assistant', id: 'final-a', content: [{ type: 'reasoning', text: 'Private reasoning' }, { type: 'text', text: 'Final ' }, { type: 'refusal', text: 'answer' }] } } },
  ], statistics: { turns: '12', steps: '34', completed_responses: '12', model_requests: '34', requests_with_usage: '34', reported_usage: { input_tokens: 1000, output_tokens: 200, total_tokens: 1200 } } } };
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
  expect(ui.getAllByLabelText('Completed Turn')).toHaveLength(1);
  expect(ui.getByLabelText('Completed Turn').closest('article')).toBeNull();
  expect(ui.getByLabelText('Completed Turn').getAttribute('data-turn-tail')).toBe(JSON.stringify(['origin-conversation','attempt-a']));
  const tail=within(ui.getByLabelText('Completed Turn'));
  await act(async()=>fireEvent.click(tail.getByRole('button',{name:'Copy'})));
  expect(writeText).toHaveBeenCalledWith('Final answer');
  fireEvent.click(tail.getByRole('button',{name:'Usage 120 tok'}));
  const detail=within(await ui.findByRole('dialog',{name:'Turn usage'}));
  expect(detail.getByText('Input')).toBeTruthy();
  expect(detail.queryByText('Cached input')).toBeNull(); expect(detail.queryByText('Cache hit')).toBeNull(); expect(detail.queryByText('Model')).toBeNull();
  expect(ui.queryByText(/Ran for/)).toBeNull();
  expect(ui.container.textContent).not.toContain('attempt-a'); expect(ui.container.textContent).not.toContain('42');
});
it('unfinalized output has no fabricated tail, usage or lineage', () => {
  const state=conversation(); delete state.transcript.entries![2].completed_response;
  const ui=render(<AgentTranscript snapshot={state} onHistorical={vi.fn()}/>);
  expect(ui.queryByLabelText('Completed Turn')).toBeNull();
  expect(ui.queryByRole('button',{name:'Lineage'})).toBeNull();
});
it('direct Turn actions forward the exact native response and keeps switching conservative', () => {
  const onHistorical=vi.fn(); const ui=render(<AgentTranscript snapshot={conversation()} onHistorical={onHistorical} lineageSwitchSafe={false}/>);
  expect(ui.queryByRole('button',{name:'Lineage'})).toBeNull();
  // Harness seats one branch action: a new Session. Retry is rustX's only other lineage action.
  expect(ui.queryByRole('button',{name:'Branch in this Session'})).toBeNull();
  expect(ui.getByRole('button',{name:'Regenerate'})).toHaveProperty('disabled',true);
  fireEvent.click(ui.getByRole('button',{name:'Branch into a new Session'}));
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
it('composer pills read native totals and the last measured request, never the loaded transcript', async () => {
  const state=conversation(); state.transcript.entries=[];
  const statistics={...state.transcript.statistics!,timing:{model_ms:20500,tool_ms:9652000,mean_ttft_ms:3100,output_tokens_per_second:49.2},
    reported_usage:{input_tokens:22297,output_tokens:543,total_tokens:22840,details:{cached_input_tokens:13568}}};
  const occupancy={input_tokens:7700,context_window_tokens:262144,model:'historical-model',breakdown:{system_tokens:1900,tool_tokens:6000,message_tokens:0}};
  act(()=>localeController.setLocale('zh'));
  const ui=render(<ConversationStats statistics={statistics} occupancy={occupancy}/>);
  fireEvent.click(ui.getByRole('button',{name:'12 轮 34 步 · 49 tok/s'}));
  const session=within(await ui.findByRole('dialog',{name:'会话统计'}));
  expect(session.getByText('20.5秒')).toBeTruthy(); expect(session.getByText('160分52秒')).toBeTruthy(); expect(session.getByText('3.1秒')).toBeTruthy();
  fireEvent.click(ui.getByRole('button',{name:'22.8K tok · 缓存命中 61%'}));
  const usage=within(await ui.findByRole('dialog',{name:'Token 用量'}));
  expect(usage.getByText('22,840 tok')).toBeTruthy(); expect(usage.getByText('8,729 tok')).toBeTruthy(); expect(usage.getByText('13,568 tok')).toBeTruthy();
  // Every request reported usage, so no coverage row is shown.
  expect(usage.queryByText('用量上报')).toBeNull();
  fireEvent.click(ui.getByRole('button',{name:'上下文已用 3%'}));
  const context=within(await ui.findByRole('dialog',{name:'上下文已用'}));
  expect(context.getByText('7.7K / 262K')).toBeTruthy(); expect(context.getByText('~6K')).toBeTruthy();
  // A zero part draws no segment: the bar keeps only the system and tool parts.
  expect(document.querySelectorAll('[role="dialog"] [style*="width"]')).toHaveLength(2);
  ui.rerender(<ConversationStats statistics={{...statistics,timing:undefined,requests_with_usage:'33',reported_usage:{input_tokens:100,output_tokens:20,total_tokens:120}}}/>);
  expect(ui.getByText('12 轮 34 步').closest('button')).toBeNull();
  expect(ui.queryByRole('button',{name:/上下文已用/})).toBeNull();
  fireEvent.click(ui.getByRole('button',{name:'120 tok'}));
  const partial=within(await ui.findByRole('dialog',{name:'Token 用量'}));
  expect(partial.getByText('输入')).toBeTruthy(); expect(partial.getByText('33/34')).toBeTruthy();
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

it('the Turn usage dialog splits cached input and names the requests\' models', async () => {
  const state = conversation();
  state.transcript.entries![2].completed_response = { ...response, models: ['kimi-k3', 'deepseek-v4'], timing: { total_duration_ms: 19000 },
    usage: { input_tokens: 15055, output_tokens: 406, total_tokens: 15461, details: { cached_input_tokens: 7168, reasoning_tokens: 120 } } };
  const ui = render(<AgentTranscript snapshot={state}/>);
  fireEvent.click(ui.getByRole('button', { name: 'Usage 15.5K tok' }));
  const detail = within(await ui.findByRole('dialog', { name: 'Turn usage' }));
  expect(detail.getByText('15,461 tok')).toBeTruthy();
  expect(detail.getByText('kimi-k3, deepseek-v4')).toBeTruthy();
  expect(detail.getByText('47.6%')).toBeTruthy();
  expect(detail.getByText('7,887 tok')).toBeTruthy();
  expect(detail.getByText('7,168 tok')).toBeTruthy();
  expect(detail.getByText('(120 tok reasoning)')).toBeTruthy();
  // The process header owns the Turn duration; the actions carry no timing pill.
  expect(ui.queryByRole('button', { name: /Ran for/ })).toBeNull();
});
