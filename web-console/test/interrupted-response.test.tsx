import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { InFlightBlock, RuntimeClientSnapshot, TurnProcessView } from '../../protocol/app-server/v57';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { conversationPreferences } from '../src/app/conversation-preferences';
import { publicationHeads } from '../src/client/publication-heads';
import { readCut, snapshot } from './fixture';

afterEach(() => { cleanup(); conversationPreferences().update({ transcriptMode: 'detailed' }); });
const turn: TurnProcessView = { conversation_id: 'conversation-A', attempt_id: 'stopped-attempt', attempt_id_source: null,
  final_message_locator: null, final_message_id_deferred: false, final_message_id_source: null,
  control_cursor: '2', outcome: 'cancelled', message_count: 0, tool_call_count: 0 };
const blocks: InFlightBlock[] = [
  { type: 'reasoning', block_index: 0, text: 'Reasoning already shown' },
  { type: 'text', block_index: 1, text: '## Unfinished answer\n\nThe preserved response.' },
];
function stopped(content = blocks): RuntimeClientSnapshot {
  return { ...snapshot(), attempt: { attempt_id: 'stopped-attempt', turn: 1, phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } },
    transcript: { entries: [
      { cursor: '2', turn_process: turn, item: { type: 'publication_audit', audit: { stream_id: 'stream', attempt_id: 'stopped-attempt', turn_id: '1', request_id: 'request', message_id: 'partial', kind: 'incomplete', settled_at: '2026-10-20T00:00:00Z',
        content: content.flatMap(block => block.type === 'text' || block.type === 'reasoning' || block.type === 'refusal' ? [{ kind: block.type, block_index: block.block_index, text: block.text ?? '' }] : []) } } },
      { cursor: '3', turn_process: turn, item: { type: 'attempt_terminal', turn } },
    ] } };
}

it.each(['compact', 'standard', 'detailed', 'verbose'] as const)('Stop preserves the streamed Markdown and open reasoning in %s mode', mode => {
  conversationPreferences().update({ transcriptMode: mode });
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), attempt: { attempt_id: 'stopped-attempt', turn: 1, phase: { type: 'running' }, in_flight: { message_id: 'partial', blocks } } }}/>);
  if (mode === 'compact' || mode === 'standard') fireEvent.click(ui.getByRole('button', { name: /^Analyzing request/ }));
  const reasoning = ui.getByRole('button', { name: /^Reasoning/ }); fireEvent.click(reasoning);
  const heading = ui.getByRole('heading', { name: 'Unfinished answer' });
  expect(ui.container.querySelector('[data-assistant-stopped]')).toBeNull();
  ui.rerender(<AgentTranscript snapshot={stopped()}/>);
  expect(ui.getByRole('heading', { name: 'Unfinished answer' })).toBe(heading);
  expect(ui.getByRole('button', { name: /^Reasoning/ })).toBe(reasoning);
  expect(reasoning.getAttribute('aria-expanded')).toBe('true');
  expect(reasoning.closest('[hidden]')).toBeNull();
  const marker = ui.container.querySelector('[data-assistant-stopped]')!;
  expect(marker.textContent).toBe('Stopped');
  expect(ui.container.querySelectorAll('[data-assistant-stopped]')).toHaveLength(1);
  expect(heading.compareDocumentPosition(marker) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(marker.closest('[hidden]')).toBeNull();
  expect(marker.closest('[data-response-reveal]')?.getAttribute('data-response-reveal')).toBe('always');
  const control = ui.getByRole('button', { name: 'Stopped' });
  expect(control.hasAttribute('disabled')).toBe(true);
  expect(ui.container.querySelector('[data-chat-running]')).toBeNull();
  expect(ui.queryByRole('button', { name: 'Regenerate' })).toBeNull();
  expect(ui.container.querySelector('[data-turn-tail]')).toBeNull();
  const next = stopped();
  next.transcript.entries!.push({ cursor: '4', item: { type: 'message', message: { role: 'user', id: 'next', source: 'human', content: [{ type: 'text', text: 'Continue after stopping' }] } } });
  ui.rerender(<AgentTranscript snapshot={JSON.parse(JSON.stringify(next))}/>);
  expect(ui.container.querySelector('[data-assistant-stopped]')).toBe(marker);
  expect(ui.getByRole('heading', { name: 'Unfinished answer' })).toBe(heading);
});

it('a frozen reasoning-only response has one stopped marker and can be expanded after reconstruction', () => {
  const state = stopped([blocks[0]!]);
  const ui = render(<AgentTranscript snapshot={JSON.parse(JSON.stringify(state))}/>);
  expect(ui.container.querySelectorAll('[data-assistant-stopped]')).toHaveLength(1);
  const reasoning = ui.getByRole('button', { name: /^Reasoning/ }); fireEvent.click(reasoning);
  expect(ui.getByText('Reasoning already shown').closest('[hidden]')).toBeNull();
});

it('trailing reasoning cannot hide the stopped marker or copy action for visible prose', () => {
  conversationPreferences().update({ transcriptMode: 'compact' });
  const ui=render(<AgentTranscript snapshot={stopped([{type:'text',block_index:0,text:'Visible partial'}, {type:'reasoning',block_index:1,text:'Later reasoning'}])}/>);
  expect(ui.container.querySelector('[data-assistant-stopped]')?.closest('[hidden]')).toBeNull();
  expect(ui.getByRole('button',{name:'Copy'}).closest('[hidden]')).toBeNull();
});

it('discarded tool proposals do not shift surviving native block indices or create assembling rows', () => {
  const state = stopped([{ type: 'reasoning', block_index: 0, text: 'Kept reasoning' }, { type: 'text', block_index: 3, text: 'Kept later text' }]);
  const entry = state.transcript.entries![0]!;
  if (entry.item.type !== 'publication_audit') throw Error('Expected native audit');
  entry.item.audit.content.splice(1, 0, { kind: 'proposed_tool_call', block_index: 1, call_id: 'proposal', tool_id: 'bash', name: 'bash', arguments: '{', complete: false });
  const ui = render(<AgentTranscript snapshot={state}/>);
  expect(ui.getByText('Kept later text')).toBeTruthy();
  expect(ui.queryByText(/Assembling/)).toBeNull();
  expect(ui.container.querySelectorAll('[data-assistant-stopped]')).toHaveLength(1);
  act(() => conversationPreferences().update({ transcriptMode: 'compact' }));
  expect(ui.getByText('Kept later text').closest('[hidden]')).toBeNull();
});

it('a native publication event replaces live output without waiting for transcript backfill', () => {
  const state = stopped(), entry = state.transcript.entries![0]!;
  if (entry.item.type !== 'publication_audit') throw Error('Expected audit');
  const audit = entry.item.audit;
  const cut = { read: readCut(), ledger_through: '0', transcript_total: '2', surface_revision: '0', inherited_through: '0' };
  const event = { type: 'assistant_publication_settled' as const, attempt_id: 'stopped-attempt', audit, transcript_cursor: '2', settlement_head: [
    { transcript_cursor: '2', read_cut: cut, settlement: { ...audit, block_count: '2' } },
  ] };
  const live = { ...snapshot(), attempt: { attempt_id: 'stopped-attempt', turn: 1, phase: { type: 'running' as const }, in_flight: { message_id: 'partial', blocks } } };
  const ui = render(<AgentTranscript snapshot={live}/>);
  const reasoning = ui.getByRole('button', { name: /^Reasoning/ }); fireEvent.click(reasoning);
  const heading = ui.getByRole('heading', { name: 'Unfinished answer' });
  // Publication settlement and execution settlement are separate native facts.
  const settling = { ...live, attempt: { ...live.attempt, in_flight: undefined }, publication_settlements: event.settlement_head };
  const heads = publicationHeads([], event, settling)!;
  expect(heads).toHaveLength(1);
  expect(publicationHeads(heads, { type: 'attempt_settled', attempt_id: 'stopped-attempt', outcome: { type: 'cancelled', reason: 'user_requested' } }, settling)).toBe(heads);
  ui.rerender(<AgentTranscript snapshot={settling} publicationHeads={heads}/>);
  expect(ui.getByRole('heading', { name: 'Unfinished answer' })).toBe(heading);
  expect(ui.getByRole('button', { name: /^Reasoning/ })).toBe(reasoning);
  expect(reasoning.getAttribute('aria-expanded')).toBe('true');
  expect(ui.container.querySelectorAll('[data-assistant-stopped]')).toHaveLength(1);
  // The native terminal cursor is after the audit; presentation seats its
  // stopped heading above the explicitly owned publication.
  const durable = { ...state, publication_settlements: event.settlement_head };
  durable.transcript.entries![1]!.item = { type: 'attempt_terminal', turn: { ...turn, control_cursor: '3' } };
  const withoutMembership = durable.transcript.entries![0]!; withoutMembership.turn_process = undefined;
  const retired = publicationHeads(heads, event, durable);
  expect(retired).toEqual([]);
  ui.rerender(<AgentTranscript snapshot={durable} publicationHeads={retired}/>);
  expect(ui.getByRole('heading', { name: 'Unfinished answer' })).toBe(heading);
  expect(ui.getByRole('button', { name: 'Stopped' }).compareDocumentPosition(heading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(publicationHeads(heads, event, { ...settling, conversation_id: 'another-conversation' })).toEqual([]);
  expect(publicationHeads(heads, { type: 'attempt_settled', attempt_id: 'stopped-attempt', outcome: { type: 'cancelled', reason: 'user_requested' } }, { ...settling, publication_settlements: [] })).toEqual([]);
  // Read-only historical windows do not receive these live native headers.
  const history = { page: { entries: [] }, epoch: 1, window: { cut: readCut(), page: { entries: [] } } };
  ui.rerender(<AgentTranscript snapshot={snapshot()} history={history} publicationHeads={heads}/>);
  expect(ui.queryByRole('heading', { name: 'Unfinished answer' })).toBeNull();
});
