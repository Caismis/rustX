import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { StopSequence } from '../src/app/composer/stop-sequence';
beforeEach(() => vi.useFakeTimers()); afterEach(() => vi.useRealTimers());
it('one press arms, an independent second within the inclusive 500 ms window dispatches once', () => {
  const sequence = new StopSequence(), scope = { authority: {}, identity: 'A' }, cancel = vi.fn();
  sequence.prepare()(scope, cancel); expect(cancel).not.toHaveBeenCalled();
  vi.advanceTimersByTime(500); sequence.prepare()(scope, cancel);
  expect(cancel).toHaveBeenCalledTimes(1); expect(vi.getTimerCount()).toBe(0);
});
it('expiry and exact scope replacement invalidate the first press', () => {
  const sequence = new StopSequence(), authority = {}, cancel = vi.fn();
  sequence.prepare()({ authority, identity: 'A' }, cancel); vi.advanceTimersByTime(501);
  expect(vi.getTimerCount()).toBe(0); sequence.prepare()({ authority, identity: 'A' }, cancel);
  sequence.prepare()({ authority, identity: 'B' }, cancel); sequence.prepare()({ authority: {}, identity: 'B' }, cancel);
  expect(cancel).not.toHaveBeenCalled(); sequence.reset(); expect(vi.getTimerCount()).toBe(0);
});
it('reset precedes reentrancy and exceptions', () => {
  const sequence = new StopSequence(), scope = { authority: {}, identity: 'A' }, inner = vi.fn();
  sequence.prepare()(scope, inner);
  expect(() => sequence.prepare()(scope, () => { expect(vi.getTimerCount()).toBe(0); sequence.prepare()(scope, inner); throw Error('failure'); })).toThrow('failure');
  expect(inner).not.toHaveBeenCalled(); sequence.reset();
});

it('foreground consumption after capture discards the old arm synchronously', () => {
  const sequence = new StopSequence(), scope = { authority: {}, identity: 'A' }, cancel = vi.fn();
  sequence.prepare()(scope, cancel); sequence.prepare();
  // The foreground owner consumed that candidate; only the next fresh event accepts.
  sequence.prepare()(scope, cancel); expect(cancel).not.toHaveBeenCalled(); sequence.reset();
});

it('focus, composition or disposal reset invalidates a captured but not yet accepted event', () => {
  const sequence = new StopSequence(), scope = { authority: {}, identity: 'A' }, cancel = vi.fn();
  sequence.prepare()(scope, cancel); const accept = sequence.prepare(); sequence.reset(); accept(scope, cancel);
  expect(cancel).not.toHaveBeenCalled(); expect(vi.getTimerCount()).toBe(0);
});
