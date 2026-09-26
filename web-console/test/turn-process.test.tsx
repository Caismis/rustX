import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { TurnProcessView, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v25';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { turnProcesses } from '../src/bindings/turn-process';
import { snapshot } from './fixture';
afterEach(cleanup);
const process = (attempt = 'a', final = 'final'): TurnProcessView => ({ conversation_id: 'c', attempt_id: attempt, final_message_id: final, outcome: 'completed', control_cursor: '1', message_count: 2, tool_call_count: 0 });
const row = (id: string, owner?: TurnProcessView, reasoning = false): RuntimeClientTranscriptEntry => ({ cursor: id, turn_process: owner, item: { type: 'message', message: { role: 'assistant', id, content: [...(reasoning ? [{ type: 'reasoning' as const, text: `Thought ${id}` }] : []), { type: 'text', text: `Answer ${id}` }] } } });
const user: RuntimeClientTranscriptEntry = { cursor: 'u', item: { type: 'message', message: { role: 'user', id: 'u', source: 'human', content: [{ type: 'text', text: 'steering' }] } } };
it('folds only native completed membership across steering and interleaved Attempts, leaving final and live work visible', () => {
  const entries = [row('first', process(), true), user, row('live', undefined, true), row('second', process(), true), row('final', process(), true)];
  const before = JSON.stringify(entries);
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), transcript: { entries } }}/>);
  const folded = screen.getByRole('button', { name: 'Worked' });
  expect(folded.getAttribute('aria-expanded')).toBe('false');
  expect(screen.getByText('Answer first').closest('[hidden]')).not.toBeNull();
  for (const text of ['Answer final', 'Answer live', 'steering']) expect(screen.getByText(text).closest('[hidden]')).toBeNull();
  fireEvent.click(folded);
  expect(screen.getByText('Answer first').closest('[hidden]')).toBeNull();
  for (const disclosure of screen.getAllByRole('button', { name: /^Reasoning/ })) fireEvent.click(disclosure);
  expect(ui.container.querySelectorAll('[data-markdown-variant="compact"]').length).toBeGreaterThanOrEqual(3);
  expect(screen.getByText('Answer final').closest('[data-markdown-variant]')?.getAttribute('data-markdown-variant')).toBe('normal');
  fireEvent.click(folded);
  expect(JSON.stringify(entries)).toBe(before);
});
it('pagination and retry origins preserve disclosure identity without merging turns', () => {
  const a = process(), b = process('retry', 'retry-final');
  const page = [row('late', a), row('final', a)];
  const key = turnProcesses(page).membership.get('late');
  const larger = turnProcesses([row('early', a), ...page, row('retry-middle', b), row('retry-final', b)]);
  expect(larger.membership.get('early')).toBe(key);
  expect(larger.membership.get('late')).toBe(key);
  expect(larger.membership.get('retry-middle')).not.toBe(key);
  expect(larger.groups.size).toBe(2);
  expect(larger.membership.has('final')).toBe(false);
  expect(larger.membership.has('retry-final')).toBe(false);
});
it.each([0, 1, 3])('counts %i exact native tool occurrences, not result bodies or display names', count => {
  const owner = { ...process(), tool_call_count: count };
  const entry = row('calls', owner);
  entry.tool_calls = Array.from({ length: count }, (_, block_index) => ({ message_id: 'calls', block_index, call_id: String(block_index), tool_id: 'bash', name: 'anything', state: { type: 'assembled', arguments: '{}' } }));
  const groups = turnProcesses([entry, row('final', owner)]);
  expect([...groups.groups.values()][0].tools).toBe(count);
});
it('a page beginning at a suppressed Tool result retains a reachable process disclosure and final answer', () => {
  const result: RuntimeClientTranscriptEntry = { cursor: 'result', turn_process: process(), item: { type: 'message', message: { role: 'tool', id: 'result', occurrence: { assistant_message_id: 'earlier-call', block_index: 0 }, tool_call_id: 'call', tool_id: 'bash', result: { status: { type: 'success' }, duration_ms: 1, content: [] } } } };
  render(<AgentTranscript snapshot={{ ...snapshot(), transcript: { entries: [result, row('final', process())] } }}/>);
  expect(screen.getByRole('button', { name: 'Worked' })).toBeTruthy();
  expect(screen.getByText('Answer final').closest('[hidden]')).toBeNull();
});
it('folds a status by exact native Attempt without moving its inbound anchor or hiding steering', () => {
  const entries = [user, row('middle', process()), row('final', process())];
  render(<AgentTranscript snapshot={{ ...snapshot(), conversation_id: 'c', transcript: { entries }, statuses: [{ attempt_id: 'a', turn: 1, status_message_id: 'status', opportunities: { fresh_inbound: { target_message_id: 'u' } }, sections: [], rendered: 'native context' }] }}/>);
  const note = screen.getByRole('note', { name: 'Agent Status', hidden: true });
  const anchor = note.closest('[data-chat-anchor-key]');
  expect(note.closest('[hidden]')).not.toBeNull();
  expect(screen.getByText('steering').closest('[hidden]')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Worked' }));
  expect(note.closest('[hidden]')).toBeNull();
  expect(note.closest('[data-chat-anchor-key]')).toBe(anchor);
  const disclosure = screen.getByRole('button', { name: 'Worked' });
  expect(screen.getByText('steering').compareDocumentPosition(disclosure) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(disclosure.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(disclosure.compareDocumentPosition(screen.getByText('Answer middle')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

it('Status-only completed process has a disclosure after its independent anchor and before the Status', () => {
  const entries = [user, row('final', process())];
  render(<AgentTranscript snapshot={{ ...snapshot(), conversation_id: 'c', transcript: { entries }, statuses: [{ attempt_id: 'a', turn: 1, status_message_id: 'only-status', opportunities: { fresh_inbound: { target_message_id: 'u' } }, sections: [], rendered: 'native context' }] }}/>);
  const disclosure = screen.getByRole('button', { name: 'Worked' });
  const note = screen.getByRole('note', { name: 'Agent Status', hidden: true });
  expect(note.closest('[hidden]')).not.toBeNull();
  expect(screen.getByText('Answer final').closest('[hidden]')).toBeNull();
  expect(screen.getByText('steering').compareDocumentPosition(disclosure) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  fireEvent.click(disclosure);
  expect(note.closest('[hidden]')).toBeNull();
  expect(disclosure.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(note.closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key')).toBe('message:u');
});
it('pagination moves only the disclosure seat and preserves its expanded native identity', () => {
  const owner = process(); const status = { attempt_id: 'a', turn: 1, status_message_id: 'status', opportunities: { fresh_inbound: { target_message_id: 'u' } }, sections: [], rendered: 'context' };
  const state = { ...snapshot(), conversation_id: 'c', statuses: [status] };
  const ui = render(<AgentTranscript snapshot={{ ...state, transcript: { entries: [row('middle', owner), row('final', owner, true)] } }}/>);
  const disclosure = screen.getByRole('button', { name: 'Worked' });
  const id = disclosure.getAttribute('data-turn-process'); fireEvent.click(disclosure);
  ui.rerender(<AgentTranscript snapshot={{ ...state, transcript: { entries: [user, row('middle', owner), row('final', owner, true), row('retry', process('retry', 'retry-final')), row('retry-final', process('retry', 'retry-final'))] } }}/>);
  const controls = screen.getAllByRole('button', { name: 'Worked' });
  const same = controls.find(button => button.getAttribute('data-turn-process') === id)!;
  expect(same.getAttribute('aria-expanded')).toBe('true');
  const note = screen.getByRole('note', { name: 'Agent Status' });
  expect(same.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(controls.find(button => button !== same)?.getAttribute('aria-expanded')).toBe('false');
  fireEvent.click(same);
  expect(screen.getByText('Answer final').closest('[hidden]')).toBeNull();
  expect(screen.queryByRole('button', { name: /^Reasoning/ })).toBeNull();
});


it.each(['failed', 'cancelled', 'timed_out', 'limit_exceeded'] as const)('native %s process owns real content, counts and a stable always-open control across pages', outcome => {
  const owner: TurnProcessView = { conversation_id: 'c', attempt_id: 'failed-native', event_id: 'terminal-native', control_cursor: '2', outcome,
    started_at: '2026-09-25T00:00:00Z', ended_at: '2026-09-25T00:00:07Z', message_count: 2, tool_call_count: 1 };
  const first = row('reasoning-and-call', owner, true); first.cursor = '2';
  if (first.item.type === 'message' && first.item.message.role === 'assistant') first.item.message.content.push({ type: 'tool_call', id: 'call', tool_id: 'bash', name: 'bash', arguments: { command: 'pwd' } });
  first.tool_calls = [{ message_id: 'reasoning-and-call', block_index: 2, call_id: 'call', tool_id: 'bash', name: 'bash', state: { type: 'assembled', arguments: '{"command":"pwd"}' } }];
  const result: RuntimeClientTranscriptEntry = { cursor: '3', turn_process: owner, item: { type: 'message', message: { role: 'tool', id: 'result', occurrence: { assistant_message_id: 'reasoning-and-call', block_index: 2 }, tool_call_id: 'call', tool_id: 'bash', result: { status: { type: 'success' }, duration_ms: 1, content: [] } } } };
  const intermediate = row('intermediate', owner); intermediate.cursor = '4';
  const terminal: RuntimeClientTranscriptEntry = { cursor: '5', turn_process: owner, item: { type: 'attempt_terminal', turn: owner } };
  const label = outcome === 'cancelled' ? 'Stopped' : 'Failed';
  const state = { ...snapshot(), conversation_id: 'c' };
  const ui = render(<AgentTranscript snapshot={{ ...state, transcript: { entries: [terminal] } }}/>);
  const control = screen.getByRole('button', { name: label });
  const identity = control.getAttribute('data-turn-process');
  expect(identity).toBe(JSON.stringify(['c', 'failed-native']));
  expect(control.getAttribute('data-turn-process-messages')).toBe('2');
  expect(control.getAttribute('data-turn-process-tool-calls')).toBe('1');
  expect(control.hasAttribute('disabled')).toBe(true);
  expect(control.getAttribute('data-open')).toBe('true');
  for (const entries of [[intermediate, terminal], [result, intermediate, terminal], [first, result, intermediate, terminal]]) {
    ui.rerender(<AgentTranscript snapshot={{ ...state, transcript: { entries } }}/>);
    expect(screen.getAllByRole('button', { name: label })).toHaveLength(1);
    expect(screen.getByRole('button', { name: label })).toBe(control);
    expect(screen.getByText('Answer intermediate').closest('[hidden]')).toBeNull();
    expect(screen.getByText('Answer intermediate').closest('[data-turn-process-owner]')?.getAttribute('data-turn-process-owner')).toBe(identity);
    expect(control.compareDocumentPosition(screen.getByText('Answer intermediate')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  }
  expect(screen.getByText('Answer reasoning-and-call').closest('[hidden]')).toBeNull();
  fireEvent.click(control);
  expect(screen.getByText('Answer intermediate').closest('[hidden]')).toBeNull();
  const reconstructed = JSON.parse(JSON.stringify([first, result, intermediate, terminal])) as RuntimeClientTranscriptEntry[];
  const later = row('later', { ...process('later', 'later'), control_cursor: '6' }); later.cursor = '6';
  ui.rerender(<AgentTranscript snapshot={{ ...state, transcript: { entries: [...reconstructed, later] } }}/>);
  expect(screen.getByRole('button', { name: label })).toBe(control);
  // An isolated member page is independently resolvable without the terminal row.
  ui.rerender(<AgentTranscript snapshot={{ ...state, transcript: { entries: [intermediate] } }}/>);
  expect(screen.getByRole('button', { name: label })).toBe(control);
  expect(control.getAttribute('data-turn-process-messages')).toBe('2');
  ui.unmount();
  render(<AgentTranscript snapshot={{ ...state, transcript: { entries: reconstructed } }}/>);
  expect(screen.getByRole('button', { name: label }).getAttribute('data-turn-process')).toBe(identity);
});
