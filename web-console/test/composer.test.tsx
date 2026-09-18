import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentComposer } from '../src/app/agent/AgentComposer';
afterEach(cleanup);
const props = () => ({ disabled: false, busy: false, active: false, onSend: vi.fn(async () => false), onUpload: vi.fn(async () => []), onCancel: vi.fn(), onCommand: vi.fn() });

it.each(['button', 'Enter', 'Control', 'Meta'])('resolves %s through the same message policy, preserving failed drafts', async gesture => {
  const p = props(); const ui = render(<AgentComposer {...p} active />);
  const input = screen.getByLabelText('Message');
  expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy();
  // Enter over the Stop seat never cancels work.
  fireEvent.keyDown(input, { key: 'Enter' }); expect(p.onCancel).not.toHaveBeenCalled();
  fireEvent.change(input, { target: { value: 'Keep the native contract' } });
  expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
  expect(ui.container.querySelectorAll('[data-composer-primary]')).toHaveLength(1);
  expect(screen.queryByLabelText('Delivery')).toBeNull();
  await act(async () => {
    if (gesture === 'button') fireEvent.click(screen.getByRole('button', { name: 'Queue' }));
    else fireEvent.keyDown(input, { key: 'Enter', ctrlKey: gesture === 'Control', metaKey: gesture === 'Meta' });
  });
  expect(p.onSend).toHaveBeenCalledExactlyOnceWith('Keep the native contract', [], ['Control', 'Meta'].includes(gesture) ? 'steer' : 'send');
  expect(input).toHaveProperty('value', 'Keep the native contract');
  ui.rerender(<AgentComposer {...p} />);
  expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
});
it('keeps one disabled Stop after cancellation request until native running changes', () => {
  const p = props(); const ui = render(<AgentComposer {...p} active />);
  fireEvent.click(screen.getByRole('button', { name: 'Stop' })); expect(p.onCancel).toHaveBeenCalledTimes(1);
  ui.rerender(<AgentComposer {...p} active cancellationAvailable={false} />);
  expect(screen.getByRole('button', { name: 'Stop' })).toHaveProperty('disabled', true);
  ui.rerender(<AgentComposer {...p} active disabled cancellationAvailable />);
  expect(screen.getByRole('button', { name: 'Stop' })).toHaveProperty('disabled', false);
});
it.each(['button', 'Enter'])('running command %s uses the selected typed command, never Queue or Steer', async gesture => {
  const p = props(); render(<AgentComposer {...p} active />);
  const input = screen.getByLabelText('Message');
  fireEvent.change(input, { target: { value: '/mdl' } });
  expect(screen.queryByRole('button', { name: 'Queue' })).toBeNull();
  await act(async () => gesture === 'button' ? fireEvent.click(screen.getByRole('button', { name: 'Run command' })) : fireEvent.keyDown(input, { key: 'Enter' }));
  expect(p.onCommand).toHaveBeenCalledExactlyOnceWith('model'); expect(p.onSend).not.toHaveBeenCalled();
  expect(input).toHaveProperty('value', '/mdl');
  fireEvent.change(input, { target: { value: '/unknown' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review command' }));
  expect(screen.getByRole('alert').textContent).toContain('Unsupported command');
  expect(input).toHaveProperty('value', '/unknown'); expect(p.onSend).not.toHaveBeenCalled();
});
it('IME, keyCode 229 and Shift+Enter cannot submit, including the command discovery path', () => {
  const p = props(); render(<AgentComposer {...p} active />);
  const input = screen.getByLabelText('Message');
  for (const draft of ['text', '/mdl']) {
    fireEvent.change(input, { target: { value: draft } });
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
    fireEvent.keyDown(input, { key: 'Enter', keyCode: 229 });
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
  }
  expect(p.onSend).not.toHaveBeenCalled(); expect(p.onCommand).not.toHaveBeenCalled();
});
it('an uploaded attachment cannot be discarded by command selection', async () => {
  const p = props(); const receipt = { session_id: 'A', batch_id: 'batch', token: 'receipt' };
  render(<AgentComposer {...p} active onUpload={async () => [{ receipt, file: { batch_id: 'batch', name: 'note.txt' }, path: '/note.txt' }]} />);
  await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['note'], 'note.txt')] } }));
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: '/model' } });
  fireEvent.click(screen.getByRole('button', { name: 'Run command' }));
  expect(screen.getByRole('alert').textContent).toContain('Remove draft attachments');
  expect(screen.getByRole('button', { name: 'Remove note.txt' })).toBeTruthy();
  expect(p.onCommand).not.toHaveBeenCalled(); expect(p.onSend).not.toHaveBeenCalled();
});
