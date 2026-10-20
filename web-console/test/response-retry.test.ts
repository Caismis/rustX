import { expect, it } from 'vitest';
import type { CompletedResponseView, RuntimeClientTranscriptEntry, TurnProcessView } from '../../protocol/app-server/v51';
import { responseRetry, responseRetryReady } from '../src/app/agent/response-retry';
import { snapshot } from './fixture';
import type { SessionView } from '../src/client/app-server';

const process: TurnProcessView = { conversation_id: 'conversation-A', attempt_id: 'original', attempt_id_source: null,
  control_cursor: '3', final_message_id_deferred: false, outcome: 'completed', tool_call_count: 1, message_count: 3 };
const response: CompletedResponseView = { origin: process, closing_locator: { type: 'by_message_id', message_id: 'reply' },
  retry_locator: { type: 'by_message_id', message_id: 'question' }, retry_message_id: 'question', closing_message_id: 'reply',
  completed_at: '2026-10-20T00:00:00Z', surface_revision: '8', deferred_fields: [] };
const row = (cursor: string, id: string, role: 'user' | 'assistant', text: string): RuntimeClientTranscriptEntry => ({
  cursor, item: { type: 'message', message: role === 'user' ? { id, role, source: 'human', content: [{ type: 'text', text }] }
    : { id, role, content: [{ type: 'text', text }] } },
});
function view() {
  const state = snapshot('A');
  state.transcript.entries = [row('1', 'previous-user', 'user', 'Earlier question'), row('2', 'previous-answer', 'assistant', 'Earlier answer'),
    { ...row('3', 'question', 'user', 'Retry this'), turn_process: process },
    { ...row('4', 'thinking', 'assistant', 'Old process'), turn_process: process },
    { ...row('5', 'reply', 'assistant', 'Old reply'), turn_process: process, completed_response: response },
    row('6', 'later-user', 'user', 'Later question')];
  state.messages = state.transcript.entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message] : []);
  return { id: 'A', snapshot: state, target: { session_id: 'A', conversation_id: 'conversation-A', attachment_id: 'old' } } as SessionView;
}

it('retains the exact question and earlier reading while removing all output and subsequent turns without mutating native state', () => {
  const source = view(), original = structuredClone(source);
  const retry = responseRetry(source, response)!;
  expect(retry.view.snapshot!.messages.map(message => message.id)).toEqual(['previous-user', 'previous-answer', 'question']);
  expect(retry.view.history!.page.entries!.map(entry => entry.cursor)).toEqual(['1', '2', '3']);
  expect(retry.view.history!.page.entries!.at(-1)?.turn_process).toBeUndefined();
  expect(retry.view.snapshot!.attempt).toBeNull();
  expect(source).toEqual(original);
});

it('uses the native process boundary when the adopted question is outside the reading window', () => {
  const source = view(); source.snapshot!.transcript.entries = source.snapshot!.transcript.entries!.filter(entry => entry.cursor !== '3');
  source.snapshot!.messages = [];
  expect(responseRetry(source, response)!.view.history!.page.entries!.map(entry => entry.cursor)).toEqual(['1', '2']);
});

it('uses the explicit canonical replay locator without requiring a duplicate message ID', () => {
  expect(responseRetry(view(), { ...response, retry_message_id: null })!.view.snapshot!.messages.at(-1)?.id).toBe('question');
});

it('never hands a reading prefix to an unrelated target, neighbouring input, or acceptance without an observed canonical input', () => {
  const source = view(), retry = responseRetry(source, response)!;
  expect(responseRetryReady(retry, source)).toBe(false);
  retry.replay = { target: { ...source.target!, attachment_id: 'new', conversation_id: 'new-conversation' }, messageId: 'accepted-user' };
  expect(responseRetryReady(retry, source)).toBe(false);
  const destination = { ...view(), target: retry.replay.target };
  expect(responseRetryReady(retry, destination)).toBe(false);
  destination.snapshot!.messages.push({ id: 'accepted-user', role: 'user', source: 'human', content: [{ type: 'text', text: 'Retry this' }] });
  expect(responseRetryReady(retry, destination)).toBe(true);
  expect(responseRetryReady(retry, undefined)).toBe(false);
});

it('retains a deferred question descriptor and accepts its exact native replay descriptor without waiting for the body', () => {
  const source = view();
  const descriptor = { identity: 'question', locator: { type: 'by_message_id' as const, message_id: 'question' }, role: 'user', kind: 'message',
    block_count: '1', tool_call_count: '0', tool_calls: [], content: [], collections: [] };
  source.snapshot!.message_descriptors = [descriptor];
  const retry = responseRetry(source, response)!;
  expect(retry.view.snapshot!.message_descriptors).toEqual([descriptor]);
  retry.replay = { target: source.target!, messageId: 'accepted-user' };
  source.snapshot!.message_descriptors = [{ ...descriptor, identity: 'accepted-user', locator: { type: 'by_message_id', message_id: 'accepted-user' } }];
  expect(responseRetryReady(retry, source)).toBe(true);
});
