/* Copyright (c) 2026 DeepSeek. MIT. Adapted interaction contracts; see PROVENANCE.md. */
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory, visibleTrace } from '../src/app/Trajectory';
import { replaceTrace } from '../src/client/trace';
import { traceEntry } from './trace-fixture';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';
let server: Server | undefined;
beforeEach(() => {
  localStorage.clear();
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(function(this: HTMLElement) {
    return Number.parseFloat((this.firstElementChild as HTMLElement)?.style.height ?? '') || 360;
  });
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value: function(this: HTMLElement, options: ScrollToOptions) { this.scrollTop = options.top ?? 0; } });
});
afterEach(() => { server?.client.disconnect(); server = undefined; cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const renderTrace = (entries = [traceEntry(0), traceEntry(1)]) => render(<Trajectory cache={replaceTrace({ entries })} loadEarlier={() => {}} latest={() => {}} />);
it('renders native Step and actual requests; inspector exposes only supported sections and unavailable facts', () => {
  renderTrace([traceEntry(0, { state: 'incomplete', timing: { started_at: '2026-09-15T00:00:00Z' } }), traceEntry(1)]);
  expect(screen.getAllByText('Step 1').length).toBe(2);
  fireEvent.click(screen.getByTitle('Request #0 · historical-model'));
  const inspector = screen.getByLabelText('Trace record inspector');
  expect(within(inspector).getByText('request-0')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Timing' }));
  expect(within(inspector).getAllByText('Unavailable')).toHaveLength(2);
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Usage' }));
  expect(within(inspector).getByText('Usage unavailable')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Input' }));
  expect(within(inspector).getAllByText(/Redacted/)).toHaveLength(2);
  expect(within(inspector).queryByRole('tab', { name: 'Attachments' })).toBeNull();
});
it('search, category filtering and folding retain native identities', () => {
  const attempt = traceEntry(5, { kind: 'attempt', request: null });
  const entries = [attempt, traceEntry(0), traceEntry(1)];
  expect(visibleTrace(entries, '', '', new Set(['attempt-a']))).toEqual([attempt]);
  expect(visibleTrace(entries, 'Request #1', '', new Set(['attempt-a']))).toEqual([entries[2]]);
  const ui = renderTrace(entries);
  fireEvent.click(screen.getByLabelText('Fold Attempt attempt-a'));
  expect(ui.container.querySelectorAll('[data-trace-id]')).toHaveLength(1);
  fireEvent.change(screen.getByLabelText('Search loaded Trace'), { target: { value: 'Request #1' } });
  expect(ui.container.querySelectorAll('[data-trace-id]')).toHaveLength(1);
  expect(screen.getByTitle('Request #1 · historical-model')).toBeDefined();
  fireEvent.change(screen.getByLabelText('Trace category'), { target: { value: 'tool' } });
  expect(screen.getByText('No matching loaded records.')).toBeDefined();
});
it('selection survives prepend by stable ID, updates payload, and reports removal on replacement', () => {
  const first = traceEntry(10, { output: [{ text: 'partial', redacted: false, truncated: true }] });
  const ui = renderTrace([first]);
  fireEvent.click(screen.getByTitle('Request #10 · historical-model'));
  ui.rerender(<Trajectory cache={replaceTrace({ entries: [traceEntry(9), first] })} loadEarlier={() => {}} latest={() => {}} />);
  let inspector = screen.getByLabelText('Trace record inspector');
  expect(within(inspector).getByText('request-10')).toBeDefined();
  fireEvent.click(within(inspector).getByRole('tab', { name: 'Output' }));
  expect(within(inspector).getByText('Content shown partially · truncated')).toBeDefined();
  ui.rerender(<Trajectory cache={replaceTrace({ entries: [traceEntry(9)] })} loadEarlier={() => {}} latest={() => {}} />);
  expect(screen.queryByLabelText('Trace record inspector')).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('Selected record left');
});
it('virtualizes a long ledger and preserves measurements on payload-only refresh', () => {
  const entries = Array.from({ length: 500 }, (_, index) => traceEntry(index));
  const ui = renderTrace(entries);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect(ledger.getAttribute('aria-rowcount')).toBe('500');
  const count = ui.container.querySelectorAll('[data-trace-id]').length;
  expect(count).toBeGreaterThan(0); expect(count).toBeLessThan(40);
  ledger.scrollTop = 9000; fireEvent.scroll(ledger);
  const top = ledger.scrollTop;
  ui.rerender(<Trajectory cache={replaceTrace({ entries: entries.map(entry => ({ ...entry, output: [{ text: 'changed', truncated: false, redacted: false }] })) })} loadEarlier={() => {}} latest={() => {}} />);
  expect(ledger.scrollTop).toBe(top);
});
it('Chat / Trajectory switching stays on one attachment while authoritative live facts change', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { entries: [traceEntry(0)] } });
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A'] }));
  render(<App client={server.client} />);
  const attachments = server.requests.filter(item => item.request.method === 'session/attach').length;
  fireEvent.click(screen.getByRole('tab', { name: 'Trajectory' }));
  await act(async () => { await server!.update('A', { ...snapshot(), trace: { entries: [traceEntry(0), traceEntry(1)] } }); });
  expect(screen.getByTitle('Request #1 · historical-model')).toBeDefined();
  fireEvent.click(screen.getByRole('tab', { name: 'Chat' }));
  expect(screen.getByLabelText('Canonical conversation')).toBeDefined();
  expect(server.requests.filter(item => item.request.method === 'session/attach')).toHaveLength(attachments);
});

it('folds one logical Step with both actual requests without hiding the next Step', () => {
  renderTrace([traceEntry(8, { kind: 'step', request: null }), traceEntry(0), traceEntry(1), traceEntry(2, { location: { attempt_id: 'attempt-a', step_id: '2' } })]);
  fireEvent.click(screen.getByLabelText('Fold Step 1'));
  expect(screen.queryByTitle('Request #0 · historical-model')).toBeNull();
  expect(screen.queryByTitle('Request #1 · historical-model')).toBeNull();
  expect(screen.getByTitle('Request #2 · historical-model')).toBeDefined();
});
it('follows appended records only at the tail, preserves the reader anchor on prepend and payload changes', () => {
  let entries = Array.from({ length: 80 }, (_, index) => traceEntry(index + 10));
  const ui = renderTrace(entries);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  const update = () => ui.rerender(<Trajectory cache={replaceTrace({ entries })} loadEarlier={() => {}} latest={() => {}} />);
  ledger.scrollTop = 80 * 36 - 360; fireEvent.scroll(ledger);
  entries = [...entries, traceEntry(90)]; update();
  expect(ledger.scrollTop).toBe(81 * 36 - 360);
  fireEvent.scroll(ledger);
  ledger.scrollTop = 720; fireEvent.scroll(ledger);
  entries = [...entries, traceEntry(91)]; update();
  expect(ledger.scrollTop).toBe(720);
  entries = [...Array.from({ length: 10 }, (_, index) => traceEntry(index)), ...entries]; update();
  expect(ledger.scrollTop).toBe(1080);
  fireEvent.scroll(ledger);
  entries = entries.map(entry => ({ ...entry, output: [{ text: 'stream changes above and below the anchor', truncated: false, redacted: false }] })); update();
  expect(ledger.scrollTop).toBe(1080);
});
