import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { Message } from '../src/app/agent/Message';
afterEach(cleanup);

it('renders canonical compaction as an accessible collapsed card with intact Markdown', () => {
  const ui = render(<Message message={{ role: 'user', id: 'summary', source: 'runtime', timestamp: '2026-10-07T00:00:00Z',
    kind: { compaction_summary: {} }, content: [{ type: 'text', text: '## Earlier work\n\nKeep  the English words.\n\n- Continue here' }] }} />);
  const trigger = ui.getByRole('button', { name: /Context summary/ });
  expect(trigger.getAttribute('aria-expanded')).toBe('false');
  expect(ui.queryByRole('heading', { name: 'Earlier work' })).toBeNull();
  expect(trigger.textContent).toContain('Keep the English words.');
  expect(trigger.textContent).not.toContain('##');
  fireEvent.keyDown(trigger, { key: 'Enter' });
  expect(trigger.getAttribute('aria-expanded')).toBe('true');
  expect(ui.getByRole('heading', { name: 'Earlier work' })).toBeTruthy();
  expect(ui.getByRole('listitem').textContent).toBe('Continue here');
  expect(ui.container.textContent).not.toContain('compaction_summary');
  fireEvent.keyDown(trigger, { key: ' ' });
  expect(trigger.getAttribute('aria-expanded')).toBe('false');
});
