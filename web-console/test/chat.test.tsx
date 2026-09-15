import { cleanup, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { Conversation, RuntimeFacts } from '../src/app/Conversation';
import { snapshot } from './fixture';
afterEach(cleanup);
const rich = '## Answer\n\n| Key | Value |\n| --- | --- |\n| one | two |\n\n- **Strong**\n\n```rust\nfn main() {}\n```\n\n$x^2$';
it('Chat uses the same rich Markdown through streaming and canonical settlement exactly once', () => {
  const live = { ...snapshot(), attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, in_flight: { message_id: 'answer', blocks: [{ type: 'text' as const, block_index: 0, text: rich }] } } };
  const ui = render(<Conversation snapshot={live} />);
  expect(ui.getByRole('heading', { name: 'Answer' })).toBeTruthy();
  expect(ui.getByRole('table')).toBeTruthy();
  const committed = { ...live, messages: [{ role: 'assistant' as const, id: 'answer', content: [{ type: 'text' as const, text: rich }] }] };
  committed.transcript = { entries: committed.messages.map(message => ({ cursor: '1', item: { type: 'message', message } })) };
  ui.rerender(<Conversation snapshot={committed} />);
  expect(ui.getAllByRole('heading', { name: 'Answer' })).toHaveLength(1);
  expect(ui.queryByLabelText('Streaming · answer')).toBeNull();
  expect(ui.getByRole('table')).toBeTruthy();
  expect(ui.container.querySelector('code')?.textContent).toContain('fn main()');
});
it('replacement drops stale partial content; incomplete Markdown stays visible', () => {
  const live = { ...snapshot(), attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, in_flight: { message_id: 'answer', blocks: [{ type: 'text' as const, block_index: 0, text: '```rust\nunfinished <tag>' }] } } };
  const ui = render(<Conversation snapshot={live} />);
  expect(ui.container.textContent).toContain('unfinished <tag>');
  ui.rerender(<Conversation snapshot={snapshot()} />);
  expect(ui.container.textContent).not.toContain('unfinished');
});
it('reasoning, refusal and native Tool identities are readable without fabricated runtime rows', () => {
  const s = snapshot(); s.messages = [{ role: 'assistant', id: 'm', content: [
    { type: 'reasoning', text: 'Reasoning fact' }, { type: 'refusal', text: 'Refusal fact' },
    { type: 'tool_call', id: 'call-1', tool_id: 'tool', name: 'Same title', arguments: {} },
    { type: 'tool_call', id: 'call-2', tool_id: 'tool', name: 'Same title', arguments: {} },
  ] }];
  s.transcript = { entries: s.messages.map(message => ({ cursor: '1', item: { type: 'message', message } })) };
  const ui = render(<><Conversation snapshot={s} /><RuntimeFacts snapshot={s} /></>);
  expect(ui.getByText('Reasoning')).toBeTruthy(); expect(ui.getByText('Refusal fact')).toBeTruthy();
  expect(ui.getByText('Tool call · call-1')).toBeTruthy(); expect(ui.getByText('Tool call · call-2')).toBeTruthy();
  expect(ui.queryByText('Subagents')).toBeNull(); expect(ui.queryByText('Workflows')).toBeNull(); expect(ui.queryByText('Todo')).toBeNull();
});
it('durable user/assistant order and tool artifact galleries follow only transcript positions', () => {
  const s = snapshot();
  s.transcript = { entries: [
    { cursor: '1', item: { type: 'message', message: { role: 'user', id: 'u', source: 'human', content: [{ type: 'text', text: 'First user' }] } } },
    { cursor: '2', item: { type: 'message', message: { role: 'assistant', id: 'a', content: [{ type: 'text', text: 'Second assistant' }] } } },
    { cursor: '3', item: { type: 'message', message: { role: 'tool', id: 't', tool_call_id: 'native-call', tool_id: 'image-tool', result: { status: { type: 'success' }, duration_ms: 1, artifacts: [{ artifact_id: 'artifact_1', name: 'tool.png', mime_type: 'image/png' }] } } } },
  ] };
  const ui = render(<Conversation snapshot={s} />);
  expect([...ui.container.querySelectorAll('[data-chat-anchor-key]')].map(node => node.getAttribute('data-chat-anchor-key'))).toEqual(['message:u', 'message:a', 'message:t']);
  expect(ui.getByText('tool.png')).toBeTruthy();
  expect(ui.container.querySelector('[data-tool-call-id="native-call"]')).toBeTruthy();
});
