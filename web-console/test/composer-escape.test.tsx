import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AgentComposer } from '../src/app/agent/AgentComposer';
beforeEach(() => vi.useFakeTimers()); afterEach(() => { cleanup(); vi.useRealTimers(); });
const props = () => ({ binding: 'binding-A', cancellationScope: { authority: {}, identity: 'generation-1/session-A/conversation-A/attempt-A' }, cancellationAvailable: true,
  disabled: false, busy: false, active: true, onSend: vi.fn(async () => false), onUpload: vi.fn(async () => []), onCancel: vi.fn(), onCommand: vi.fn() });
function mount(p = props()) {
  const ui = render(<AgentComposer {...p} />); const input = screen.getByLabelText('Message'); act(() => input.focus());
  return { p, ui, input, escape: (patch = {}) => fireEvent.keyDown(input, { key: 'Escape', ...patch }) };
}
it('two deliberate Escape presses cancel exactly once without changing draft or selection', () => {
  const { p, input, escape } = mount(); fireEvent.change(input, { target: { value: 'retained draft' } });
  (input as HTMLTextAreaElement).setSelectionRange(2, 5);
  escape(); expect(p.onCancel).not.toHaveBeenCalled(); act(() => vi.advanceTimersByTime(499)); escape();
  expect(p.onCancel).toHaveBeenCalledTimes(1); expect(input).toHaveProperty('value', 'retained draft');
  expect([(input as HTMLTextAreaElement).selectionStart, (input as HTMLTextAreaElement).selectionEnd]).toEqual([2, 5]);
  expect(vi.getTimerCount()).toBe(0);
});
it('expired first press does not cancel', () => { const { p, escape } = mount(); escape(); act(() => vi.advanceTimersByTime(501)); escape(); expect(p.onCancel).not.toHaveBeenCalled(); });
it.each([{ repeat: true }, { isComposing: true }, { keyCode: 229 }, { ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }])('ineligible second press %j resets, without completing a sequence', patch => {
  const { p, escape } = mount(); escape(); escape(patch); escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it('consumed Escape resets instead of completing or arming', () => {
  const { p, input, escape } = mount(); escape();
  const consume = (event: Event) => event.preventDefault(); input.addEventListener('keydown', consume);
  escape(); input.removeEventListener('keydown', consume); escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it('composition beginning invalidates an earlier press and protects Enter', () => {
  const { p, input, escape } = mount(); escape(); fireEvent.compositionStart(input); escape();
  fireEvent.keyDown(input, { key: 'Enter' }); fireEvent.compositionEnd(input); escape();
  expect(p.onCancel).not.toHaveBeenCalled(); expect(p.onSend).not.toHaveBeenCalled();
});
it('command-menu Escape dismisses only and never carries an earlier arm', () => {
  const { p, input, escape } = mount(); escape(); fireEvent.change(input, { target: { value: '/mdl' } });
  expect(screen.getByRole('listbox', { name: 'Commands' })).toBeTruthy(); escape();
  expect(screen.queryByRole('listbox')).toBeNull(); expect(input).toHaveProperty('value', '/mdl');
  escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it.each(['unavailable', 'inactive', 'no Attempt'] as const)('%s scope cannot cancel', mode => {
  const p = props(); const { escape } = mount({ ...p, cancellationAvailable: mode !== 'unavailable', active: mode !== 'inactive', cancellationScope: mode === 'no Attempt' ? undefined! : p.cancellationScope });
  escape(); escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it('focus outside the editor and window blur invalidate old arms', () => {
  const { p, input, escape } = mount(); escape(); act(() => screen.getByRole('button', { name: 'Add' }).focus());
  escape(); act(() => input.focus()); escape(); expect(p.onCancel).not.toHaveBeenCalled();
  fireEvent(window, new Event('blur')); escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it.each(['binding', 'authority', 'generation', 'Session', 'Conversation', 'Attempt', 'unavailable'] as const)('%s replacement invalidates the first press even if running stays true', change => {
  const { p, ui, escape } = mount(); escape();
  const next = { ...p, binding: change === 'binding' ? 'binding-B' : p.binding,
    cancellationAvailable: change !== 'unavailable', cancellationScope: { authority: change === 'authority' ? {} : p.cancellationScope.authority,
      identity: ['binding', 'authority', 'unavailable'].includes(change) ? p.cancellationScope.identity : `changed-${change}` } };
  ui.rerender(<AgentComposer {...next} />);
  if (change === 'unavailable') ui.rerender(<AgentComposer {...p} />);
  escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it('Attempt A settlement followed by Attempt B cannot complete A’s sequence', () => {
  const { p, ui, escape } = mount(); escape(); ui.rerender(<AgentComposer {...p} active={false} cancellationAvailable={false} cancellationScope={undefined} />);
  ui.rerender(<AgentComposer {...p} cancellationScope={{ ...p.cancellationScope, identity: 'attempt-B' }} />); escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
it('unmount disposes the expiry timer and reset listener', () => {
  const remove = vi.spyOn(window, 'removeEventListener'); const { p, ui, escape } = mount(); act(() => vi.advanceTimersByTime(0)); escape(); expect(vi.getTimerCount()).toBe(1);
  ui.unmount(); expect(vi.getTimerCount()).toBe(0); expect(remove).toHaveBeenCalledWith('blur', expect.any(Function));
  act(() => vi.advanceTimersByTime(501)); expect(p.onCancel).not.toHaveBeenCalled(); remove.mockRestore();
});

it('foreground stopPropagation prevents completion and invalidates the old arm', async () => {
  const { p, input, escape } = mount(); escape();
  const consume = (event: Event) => { event.preventDefault(); event.stopPropagation(); };
  input.addEventListener('keydown', consume);
  await act(async () => escape()); input.removeEventListener('keydown', consume);
  escape(); expect(p.onCancel).not.toHaveBeenCalled();
});
