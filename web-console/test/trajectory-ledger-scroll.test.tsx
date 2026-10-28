import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TrajectoryLedger } from '../src/app/trajectory/TrajectoryLedger';
import type { TrajectoryLedgerRow } from '../src/app/trajectory/layout';
import type { TrajectoryRowActions } from '../src/app/trajectory/TrajectoryRow';
const originalScrollTo = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollTo');
beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(500);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(500);
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value: function (this: HTMLElement, options: ScrollToOptions | number) {
    this.scrollTop = typeof options === 'number' ? options : options.top ?? 0;
  } });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); if (originalScrollTo) Object.defineProperty(HTMLElement.prototype, 'scrollTo', originalScrollTo); else Reflect.deleteProperty(HTMLElement.prototype, 'scrollTo'); });
const actions: TrajectoryRowActions = { select: () => {}, toggleTurn: () => {}, toggleCalls: () => {}, foldableTurns: new Set(), callOwners: new Set(), requestNumbers: new Map() };
const rows = (count: number): TrajectoryLedgerRow[] => Array.from({ length: count }, (_, index) => ({ display_key: 'row-' + index, kind: 'summary', height: 30, turnStart: false, callsOwner: 'message-' + index, summary: 'Record ' + index }));
function fixture(count: number) {
  let current = rows(count);
  const ledger = () => <TrajectoryLedger ref={null} rows={current} first="row-0" onFocused={() => {}} folded={new Set()} focusedIds={null} actions={actions} close={() => {}} loadEarlier={() => {}} canLoadEarlier={false} loading={false} searching={false}/>;
  const ui = render(ledger()), pane = ui.getByRole('table');
  let offset = 0;
  Object.defineProperty(pane, 'scrollHeight', { configurable: true, get: () => current.length * 30 });
  Object.defineProperty(pane, 'scrollTop', { configurable: true, get: () => offset, set: (value: number) => { offset = Math.max(0, Math.min(value, current.length * 30 - 500)); } });
  return { pane, update: async (count: number) => { current = rows(count); await act(async () => ui.rerender(ledger())); } };
}
it('preserves off-tail reading position across 100 → 101 appends and fold/filter transitions through the threshold', async () => {
  const f = fixture(100); f.pane.scrollTop = 900; fireEvent.scroll(f.pane);
  await f.update(100); expect(f.pane.scrollTop).toBe(900);
  await f.update(101); expect(f.pane.scrollTop).toBe(900);
  await f.update(90); expect(f.pane.scrollTop).toBe(900);
  await f.update(101); expect(f.pane.scrollTop).toBe(900);
});
it('continues following the tail when an append enables virtualization', async () => {
  const f = fixture(100); f.pane.scrollTop = 2500; fireEvent.scroll(f.pane);
  await f.update(101); expect(f.pane.scrollTop).toBe(2530);
});
