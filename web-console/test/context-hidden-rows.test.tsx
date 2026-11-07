import { cleanup, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { MessageBlock, RuntimeClientSnapshot, RuntimeClientTranscriptEntry, UserMessageBlock } from '../../protocol/app-server/v74';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { conversationPreferences } from '../src/app/conversation-preferences';
import { snapshot } from './fixture';

afterEach(() => { cleanup(); conversationPreferences().update({ transcriptMode: 'detailed' }); });

const user = (id: string, kind: UserMessageBlock['kind'], text: string): MessageBlock =>
  ({ role: 'user', id, source: kind === 'message' ? 'human' : 'runtime', kind, content: [{ type: 'text', text }] });
const entry = (cursor: string, message: MessageBlock): RuntimeClientTranscriptEntry => ({ cursor, item: { type: 'message', message } });

/** Harness hidden rows: ordinary Context injections never reach Chat — not from
 * the loaded transcript, not as not-yet-durable current context — in any
 * work-detail mode. A Goal continuation is not a Context injection and stays
 * visible. */
it.each(['compact', 'standard', 'detailed', 'verbose'] as const)('ordinary Context stays hidden in %s mode', mode => {
  conversationPreferences().update({ transcriptMode: mode });
  const live: RuntimeClientSnapshot = {
    ...snapshot(),
    transcript: { entries: [
      entry('1', user('u1', 'message', 'Ask the question')),
      entry('2', user('ctx-durable', { context: 'extension_environment' }, 'Durable environment context')),
      entry('3', user('ctx-native', { context: 'native_environment' }, 'Native environment context')),
    ] },
    messages: [
      user('ctx-pending', { context: 'extension_environment' }, 'Pending environment context'),
      user('goal-pending', { goal_continuation: { id: 'goal-1', revision: '1' } }, 'Continue the goal'),
    ],
  };
  const ui = render(<AgentTranscript snapshot={live}/>);
  const text = ui.container.textContent ?? '';
  expect(text).toContain('Ask the question');
  for (const hidden of ['Durable environment context', 'Native environment context', 'Pending environment context', 'extension_environment', 'native_environment'])
    expect(text).not.toContain(hidden);
  expect(text).toContain('goal_continuation');
});
