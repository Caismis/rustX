import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { AssistantContentBlock, RuntimeClientTranscriptEntry, TurnProcessView } from '../../protocol/app-server/v37';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { stepGroups } from '../src/bindings/step-groups';
import { localeController } from '../src/locale/controller';
import { snapshot } from './fixture';
afterEach(() => { cleanup(); act(() => localeController.setLocale('en')); });

const owner = (outcome: TurnProcessView['outcome'] = 'completed'): TurnProcessView => ({ conversation_id: 'c', attempt_id: 'a', final_message_id: 'final', outcome, control_cursor: '1', message_count: 4, tool_call_count: 3 });
const call = (id: string, name: string): AssistantContentBlock => ({ type: 'tool_call', id, tool_id: `tool-${name}`, name, arguments: {} });
const assistant = (id: string, process: TurnProcessView, content: AssistantContentBlock[]): RuntimeClientTranscriptEntry =>
  ({ cursor: id, turn_process: process, item: { type: 'message', message: { role: 'assistant', id, content } } });
const result = (id: string, process: TurnProcessView): RuntimeClientTranscriptEntry =>
  ({ cursor: id, turn_process: process, item: { type: 'message', message: { role: 'tool', id, occurrence: { assistant_message_id: 'opening', block_index: 2 }, tool_call_id: 'read-1', tool_id: 'tool-read', result: { status: { type: 'success' }, duration_ms: 1, content: [] } } } });
// Interaction audits carry their Attempt, not process membership.
const audit = (): RuntimeClientTranscriptEntry => ({ cursor: 'audit', item: { type: 'interaction_requested', event_id: 'approval-event', timestamp: '2026-10-06T00:00:00Z', attempt_id: 'a', turn_id: 't', interaction_id: 'approval',
  subject: { type: 'approval', invocation_id: { caller: 'agent', call_id: 'bash-1' }, tool_id: 'tool-bash', tool_name: 'bash', arguments_digest: 'digest', reason: 'Developer approval required' } } });
function turn(process = owner()) {
  return [
    assistant('opening', process, [{ type: 'reasoning', text: 'Plan the change' }, { type: 'text', text: 'Let me look first.' }, call('read-1', 'read'), call('bash-1', 'bash')]),
    result('read-result', process), audit(),
    assistant('asking', process, [{ type: 'reasoning', text: 'Ask the user' }, call('ask-1', 'ask_user'), call('read-2', 'read')]),
    assistant('final', process, [{ type: 'reasoning', text: 'User picked flying' }, { type: 'text', text: 'Flying it is.' }]),
  ];
}

it('a settled Attempt groups reasoning, calls and bodied records between replies without reordering them', () => {
  const pieces = stepGroups(turn(), () => true);
  const opening = pieces.get('opening')!;
  expect(opening.map(piece => piece.kind)).toEqual(['group', 'reply', 'group']);
  expect(opening[0].kind === 'group' && opening[0].group.members).toEqual([{ entry: turn()[0], blocks: [0] }]);
  expect(opening[1]).toEqual({ kind: 'reply', blocks: [1] });
  const process = opening[2].kind === 'group' ? opening[2].group : undefined;
  expect(process?.members.map(member => [member.entry.cursor, member.blocks])).toEqual([['opening', [2, 3]], ['audit', undefined], ['asking', [0, 1, 2]], ['final', [0]]]);
  // Distinct calls rank categories; ties keep first appearance.
  expect(process?.counts).toEqual([{ kind: 'read', count: 2 }, { kind: 'commands', count: 1 }, { kind: 'questions', count: 1 }]);
  expect(pieces.has('read-result')).toBe(false);
  expect(pieces.get('audit')).toEqual([]);
  expect(pieces.get('asking')).toEqual([]);
  expect(pieces.get('final')).toEqual([{ kind: 'reply', blocks: [1] }]);
  // A live Attempt keeps every row in place.
  expect(stepGroups(turn(owner('running')), () => true).size).toBe(0);
});

it('an independent message closes the open group', () => {
  const process = owner();
  const steering: RuntimeClientTranscriptEntry = { cursor: 'steer', turn_process: process, item: { type: 'message', message: { role: 'user', id: 'steer', source: 'human', content: [{ type: 'text', text: 'Use the faster path' }] } } };
  const pieces = stepGroups([assistant('one', process, [call('read-1', 'read')]), steering, assistant('final', process, [call('grep-1', 'grep'), { type: 'text', text: 'Done.' }])], () => true);
  expect(pieces.get('one')?.map(piece => piece.kind === 'group' && piece.group.members.length)).toEqual([1]);
  expect(pieces.has('steer')).toBe(false);
  expect(pieces.get('final')?.map(piece => piece.kind)).toEqual(['group', 'reply']);
});

it('the expanded settled turn shows Harness group titles that open their members in place', () => {
  const entries = turn();
  render(<AgentTranscript snapshot={{ ...snapshot(), transcript: { entries } }}/>);
  fireEvent.click(screen.getByRole('button', { name: 'Worked' }));
  expect(screen.getByRole('button', { name: 'Analysis completed' })).toBeTruthy();
  const process = screen.getByRole('button', { name: 'Read files, ran commands, asked questions' });
  expect(process.getAttribute('aria-expanded')).toBe('false');
  expect(screen.getByText('Let me look first.').closest('[hidden]')).toBeNull();
  expect(screen.getByText('Flying it is.').closest('[hidden]')).toBeNull();
  expect(screen.getByText('Assembling ask_user…').closest('[hidden]')).not.toBeNull();
  fireEvent.click(process);
  expect(process.getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByText('Assembling ask_user…').closest('[hidden]')).toBeNull();
  expect(screen.getByText('Historical interaction details').closest('[hidden]')).toBeNull();
  // Canonical order survives grouping: opening reply, its process, then the final reply.
  const text = document.body.textContent!;
  expect(text.indexOf('Let me look first.')).toBeLessThan(text.indexOf('Assembling read'));
  expect(text.indexOf('Assembling ask_user')).toBeLessThan(text.indexOf('Flying it is.'));
  act(() => localeController.setLocale('zh'));
  expect(screen.getByRole('button', { name: '已读取文件，执行了命令，向用户提出了问题' })).toBeTruthy();
  expect(screen.getByRole('button', { name: '已完成分析' })).toBeTruthy();
});

it('two categories join with their shared Chinese prefix once', () => {
  act(() => localeController.setLocale('zh'));
  const process = owner();
  render(<AgentTranscript snapshot={{ ...snapshot(), transcript: { entries: [{ ...assistant('one', { ...process, outcome: 'failed' }, [call('read-1', 'read'), call('grep-1', 'grep'), { type: 'text', text: 'Done.' }]), cursor: '1' }] } }}/>);
  expect(screen.getByRole('button', { name: '已读取文件并搜索代码' })).toBeTruthy();
});
