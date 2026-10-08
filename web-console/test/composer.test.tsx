import { capabilities as uploadCapabilities } from './fixture';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { composerPreferences, COMPOSER_PREFERENCE_KEY } from '../src/app/composer/preferences';
beforeEach(() => { localStorage.clear(); composerPreferences().setBusyEnter('queue'); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });
const props = () => ({ disabled: false, busy: false, active: false, onSend: vi.fn(async () => false), onUpload: vi.fn(async () => []), onCancel: vi.fn(), onCommand: vi.fn() });

it.each(['button', 'Enter', 'Control', 'Meta'])('resolves %s through the same message policy, preserving failed drafts', async gesture => {
  const p = props(); const ui = render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />);
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
  expect(p.onSend).toHaveBeenCalledExactlyOnceWith('Keep the native contract', [], ['Control', 'Meta'].includes(gesture) ? 'steer' : 'send', expect.any(Function));
  expect(input).toHaveProperty('value', 'Keep the native contract');
  ui.rerender(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} />);
  expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
});
it('keeps one disabled Stop after cancellation request until native running changes', () => {
  const p = props(); const ui = render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />);
  fireEvent.click(screen.getByRole('button', { name: 'Stop' })); expect(p.onCancel).toHaveBeenCalledTimes(1);
  ui.rerender(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active cancellationAvailable={false} />);
  expect(screen.getByRole('button', { name: 'Stop' })).toHaveProperty('disabled', true);
  ui.rerender(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active disabled cancellationAvailable />);
  expect(screen.getByRole('button', { name: 'Stop' })).toHaveProperty('disabled', false);
});
it.each(['button', 'Enter'])('running command %s uses the selected typed command, never Queue or Steer', async gesture => {
  const p = props(); render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />);
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
  const p = props(); render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />);
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
  render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active onUpload={async () => [{ receipt, file: { batch_id: 'batch', name: 'note.txt' }, path: '/note.txt' }]} />);
  await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['note'], 'note.txt')] } }));
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: '/model' } });
  fireEvent.click(screen.getByRole('button', { name: 'Run command' }));
  expect(screen.getByRole('alert').textContent).toContain('Remove draft attachments');
  expect(screen.getByRole('button', { name: 'Remove note.txt' })).toBeTruthy();
  expect(p.onCommand).not.toHaveBeenCalled(); expect(p.onSend).not.toHaveBeenCalled();
});

// The preference is a General Settings row (Harness `composer-enter`); the
// Composer only reads it and renders no selector of its own.
const preferSteer = () => {
  expect(screen.queryByRole('button', { name: /Enter while running/ })).toBeNull();
  act(() => composerPreferences().setBusyEnter('steer'));
};
it.each(['button', 'Enter', 'Control', 'Meta'])('Steer preference resolves %s and persists across remounts', async gesture => {
  const p = props(); const ui = render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />); preferSteer();
  expect(localStorage.getItem(COMPOSER_PREFERENCE_KEY)).toBe('steer');
  ui.unmount(); render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />);
  const input = screen.getByLabelText('Message'); fireEvent.change(input, { target: { value: 'exact draft' } });
  expect(screen.getByRole('button', { name: 'Steer' }).title).toBe('Steer · Enter (Ctrl/Cmd+Enter to Queue)');
  await act(async () => gesture === 'button' ? fireEvent.click(screen.getByRole('button', { name: 'Steer' }))
    : fireEvent.keyDown(input, { key: 'Enter', ctrlKey: gesture === 'Control', metaKey: gesture === 'Meta' }));
  expect(p.onSend).toHaveBeenCalledExactlyOnceWith('exact draft', [], ['Control', 'Meta'].includes(gesture) ? 'send' : 'steer', expect.any(Function));
});
it.each(['button', 'Enter', 'Control', 'Meta'])('idle %s stays Send with Steer preference', async gesture => {
  const p = props(); render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} />); preferSteer();
  const input = screen.getByLabelText('Message'); fireEvent.change(input, { target: { value: 'idle' } });
  await act(async () => gesture === 'button' ? fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    : fireEvent.keyDown(input, { key: 'Enter', ctrlKey: gesture === 'Control', metaKey: gesture === 'Meta' }));
  expect(p.onSend).toHaveBeenCalledExactlyOnceWith('idle', [], 'send', expect.any(Function));
});
it('Steer preference never turns a command or IME Enter into a prompt', () => {
  const p = props(); render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />); preferSteer();
  const input = screen.getByLabelText('Message'); fireEvent.change(input, { target: { value: '/mdl' } });
  fireEvent.keyDown(input, { key: 'Enter', isComposing: true }); fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
  expect(p.onCommand).not.toHaveBeenCalled(); fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
  expect(p.onCommand).toHaveBeenCalledExactlyOnceWith('model'); expect(p.onSend).not.toHaveBeenCalled();
});
it.each(['busy', 'disabled', 'submitDisabled', 'upload'] as const)('Steer preference preserves the %s admission gate', async gate => {
  const p = props(); const ui = render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />); preferSteer();
  const input = screen.getByLabelText('Message'); fireEvent.change(input, { target: { value: 'gated' } });
  let complete!: (value: []) => void;
  if (gate === 'upload') {
    ui.rerender(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active onUpload={() => new Promise(resolve => { complete = resolve; })} />);
    fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['x'], 'gated.txt')] } });
  } else ui.rerender(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active {...{ [gate]: true }} />);
  fireEvent.keyDown(input, { key: 'Enter' }); fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
  fireEvent.keyDown(input, { key: 'Enter', metaKey: true });
  expect(p.onSend).not.toHaveBeenCalled(); expect(input).toHaveProperty('value', 'gated');
  if (gate === 'upload') await act(async () => complete([]));
});
it('denied preference writes keep the Composer usable', async () => {
  const p = props(); render(<AgentComposer uploadPolicy={uploadCapabilities.upload_policy} {...p} active />);
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw Error('denied'); }); preferSteer();
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'usable' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Steer' })));
  expect(p.onSend).toHaveBeenCalledExactlyOnceWith('usable', [], 'steer', expect.any(Function));
});
