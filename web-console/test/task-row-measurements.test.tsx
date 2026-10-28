import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TaskManagerPage } from '../src/app/scheduler/TaskManagerPage';
import { schedulerFixture, task } from './scheduler-fixture';
const originalScrollTo = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollTo');
beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) { return this.tagName === 'LI' ? this.textContent?.includes('Tall task') ? 120 : 60 : 500; });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(700);
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value: function (this: HTMLElement, value: ScrollToOptions) { this.scrollTop = value.top ?? 0; } });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); if (originalScrollTo) Object.defineProperty(HTMLElement.prototype, 'scrollTo', originalScrollTo); else Reflect.deleteProperty(HTMLElement.prototype, 'scrollTo'); });
it('keeps differently sized mounted task measurements with their identities after a next-due reorder', async () => {
  const tall = task('Tall task'), short = task('Short task', 'A', tall.binding.node_id, '2');
  short.next_due_utc = '2028-11-02T01:00:00Z';
  const f = schedulerFixture([tall, short]); await f.server.connect();
  try {
    const ui = render(<TaskManagerPage client={f.server.client} newTask={() => {}} openSession={() => {}}/>);
    await waitFor(() => expect(ui.container.querySelectorAll('li')).toHaveLength(2));
    const row = (title: string) => ui.getByRole('button', { name: new RegExp(title) }).closest('li')!;
    const tallNode = row('Tall task'), shortNode = row('Short task');
    await waitFor(() => expect(shortNode.style.transform).toBe('translateY(122px)'));
    f.rows.set(tall.id.ordinal, { ...tall, revision: String(BigInt(tall.revision) + 1n), next_due_utc: '2028-11-03T01:00:00Z' });
    await act(async () => f.notify());
    await waitFor(() => expect(shortNode.style.transform).toBe('translateY(0px)'));
    expect(row('Tall task')).toBe(tallNode); expect(row('Short task')).toBe(shortNode);
    expect(tallNode.style.transform).toBe('translateY(62px)');
  } finally { f.server.client.dispose(); }
});
