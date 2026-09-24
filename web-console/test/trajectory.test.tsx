/* Copyright (c) 2026 DeepSeek. MIT. Adapted interaction contracts; see PROVENANCE.md. */
import { useCallback, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { beginTraceDetail, completeTraceDetail, replaceTrace, selectTrace, type TraceCache } from '../src/client/trace';
import { trajectoryItems, visibleItems, matchingCalls, preferredItem, selectionOf, systemLabel, type OwnedDisplayItem } from '../src/app/trajectory/layout';
import { searchItems } from '../src/app/trajectory/search';
import { requestDetail, toolDetail, traceRecord, traceTool } from './trace-fixture';
import type { TraceContextPresentation, TraceDetail, TraceRecord } from '../../protocol/app-server/v20';

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(360);
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value: function (this: HTMLElement, options: ScrollToOptions) { this.scrollTop = options.top ?? 0; } });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const noop = () => {};
const cacheOf = (records: TraceRecord[]) => replaceTrace({ records, next_cursor: null });
function show(cache: TraceCache, load = vi.fn(), older = vi.fn()) {
  return render(<Trajectory cache={cache} loadEarlier={older} latest={noop} onSelect={noop} onLoadDetail={load} />);
}
const context = (id: string): TraceContextPresentation => ({ message_id: id, producer: { Native: 'workspace_instructions' }, context_kind: 'native_environment', source: { type: 'runtime' }, preview: { text: `Preview ${id}`, truncated: false }, attachments: [], truncated: false });
const richRequest = (n = 0) => {
  const record = traceRecord(n);
  record.request!.system_prompt = { state: 'initial', preview: { text: 'Frozen prompt', truncated: false } };
  record.request!.tool_catalog = 'initial';
  record.request!.predecessor = { availability: 'not_applicable' };
  record.request!.context_additions = [context('context-z'), context('context-a')];
  return record;
};
function row(type: string, id = 'trace:0') { return document.querySelector<HTMLElement>(`[data-display-type="${type}"][data-owner="${id}"]`)!; }

it('T1-01 maps every native prompt/tool combination without reading details', () => {
  for (const [prompt, tools, label, facet] of [
    ['initial', 'initial', 'Initial System Prompt', 'System Prompt'],
    ['changed', 'unchanged', 'System Prompt Updated', 'System Prompt'],
    ['unchanged', 'changed', 'Tools Updated', 'Tools'],
    ['changed', 'changed', 'System Prompt and Tools Updated', 'Summary'],
    ['unchanged', 'unchanged', undefined, undefined],
    ['previous_unavailable', 'previous_unavailable', 'Previous input unavailable', 'Summary'],
  ] as const) {
    const record = richRequest(); record.request!.system_prompt.state = prompt; record.request!.tool_catalog = tools;
    expect(systemLabel(record)).toEqual(label ? { label, facet } : undefined);
    const items = trajectoryItems([record]);
    expect(items.filter(item => item.type === 'SystemRow')).toHaveLength(label ? 1 : 0);
  }
});

it('T1-04 preserves frozen Context order and exact owner/display/facet identities', () => {
  const record = richRequest();
  const items = trajectoryItems([record]);
  expect(items.map(item => item.type)).toEqual(['AttemptSectionHeader', 'StepHeader', 'SystemRow', 'ContextRow', 'ContextRow', 'RequestBoundary']);
  const contexts = items.filter(item => item.type === 'ContextRow');
  expect(contexts.map(item => item.context_message_id)).toEqual(['context-z', 'context-a']);
  expect(contexts.map(item => item.owner_record_id)).toEqual(['trace:0', 'trace:0']);
  expect(new Set(items.map(item => item.display_key)).size).toBe(items.length);
  const load = vi.fn(); show(cacheOf([record]), load);
  fireEvent.click(row('SystemRow')); expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
  fireEvent.click(document.querySelector('[data-display-type="ContextRow"][data-display-key*="context-a"]')!);
  expect(screen.getByRole('tab', { name: 'Context' }).getAttribute('aria-selected')).toBe('true');
  expect(document.querySelector('[data-context-message-id="context-a"][data-selected]')).not.toBeNull();
  fireEvent.click(row('RequestBoundary')); expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
});

it('T1-04 controlled out-of-order owner responses cannot change a newer facet or steal focus', async () => {
  const resolvers = new Map<string, (value: TraceDetail) => void>();
  const reads: string[] = [];
  function Fixture() {
    const [cache, setCache] = useState(cacheOf([richRequest(0), richRequest(1)]));
    const load = useCallback((id: string) => {
      reads.push(id);
      setCache(current => beginTraceDetail(current, id));
      void new Promise<TraceDetail>(resolve => resolvers.set(id, resolve)).then(detail => setCache(current => completeTraceDetail(current, id, 1, detail)));
    }, []);
    return <Trajectory cache={cache} onSelect={id => setCache(current => selectTrace(current, id))} onLoadDetail={load} loadEarlier={noop} latest={noop} />;
  }
  render(<Fixture />);
  fireEvent.click(row('SystemRow'));
  fireEvent.click(row('RequestBoundary', 'trace:1'));
  fireEvent.click(row('ContextRow', 'trace:1'));
  const selected = row('ContextRow', 'trace:1'); selected.focus();
  expect(reads).toEqual(['trace:0', 'trace:1']);
  await act(async () => { resolvers.get('trace:1')!(requestDetail(1)); });
  await act(async () => { resolvers.get('trace:0')!(requestDetail(0)); });
  expect(screen.getByRole('tab', { name: 'Context' }).getAttribute('aria-selected')).toBe('true');
  expect(selected.getAttribute('aria-selected')).toBe('true');
  expect(document.activeElement).toBe(selected);
  fireEvent.click(row('SystemRow', 'trace:1'));
  fireEvent.click(row('RequestBoundary', 'trace:1'));
  expect(reads).toEqual(['trace:0', 'trace:1']);
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
});

it('T1-05 latest unchanged-only page directly discovers prompt with one lazy owner read', () => {
  const load = vi.fn(); show(cacheOf([traceRecord(9), traceRecord(10)]), load);
  expect(load).not.toHaveBeenCalled(); expect(document.querySelector('[data-display-type="SystemRow"]')).toBeNull();
  fireEvent.click(row('RequestBoundary', 'trace:10'));
  fireEvent.click(screen.getByRole('button', { name: 'View System Prompt' }));
  expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
  expect(load.mock.calls).toEqual([['trace:10']]);
});

it.each([[false, true], [true, false], [true, true]])('T1-02 independent truncation previous=%s current=%s forbids complete diff', (previous, current) => {
  const record = richRequest(); record.request!.system_prompt.state = 'changed';
  const detail = requestDetail(0); detail.request!.previous_system_prompt!.truncated = previous; detail.request!.effective_system_prompt.truncated = current;
  detail.request!.previous_system_prompt!.text = detail.request!.effective_system_prompt.text;
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(row('SystemRow')); fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
  expect(screen.getByText(/A complete diff cannot be produced:/)).toBeDefined();
  expect(screen.queryByText(/No changes/)).toBeNull();
});

it('T1-02 diff distinguishes initial, unavailable, empty and complete changed content', () => {
  const record = richRequest(); record.request!.system_prompt.state = 'changed';
  const detail = requestDetail(0); detail.request!.previous_system_prompt = { text: '', truncated: false }; detail.request!.effective_system_prompt = { text: 'new prompt\n', truncated: false };
  const cache = completeTraceDetail(cacheOf([record]), record.id, 1, detail);
  const view = show(cache); fireEvent.click(row('SystemRow')); fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
  expect(screen.getByLabelText('System prompt diff').textContent).toBe('+ new prompt\n');
  const update = (availability: 'not_applicable' | 'unavailable') => {
    const next = requestDetail(0); next.request!.previous_system_prompt = null;
    next.request!.predecessor = availability === 'not_applicable' ? { availability } : { availability, request_id: 'previous' };
    view.rerender(<Trajectory cache={completeTraceDetail(cache, record.id, 1, next)} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  };
  update('not_applicable'); expect(screen.getByText('No predecessor · initial prompt.')).toBeDefined();
  update('unavailable'); expect(screen.getByText(/Previous prompt unavailable/)).toBeDefined();
});

it('T1-06 retries stay one logical Step; failed and running requests need no Assistant', () => {
  const records = ['failed', 'failed', 'running', 'completed'].map((state, n) => traceRecord(n, { state: state as TraceRecord['state'] }));
  const items = trajectoryItems(records);
  expect(items.filter(item => item.type === 'StepHeader')).toHaveLength(1);
  expect(items.filter(item => item.type === 'RequestBoundary').map(item => item.record.request!.retry_number)).toEqual([0, 1, 2, 3]);
  show(cacheOf(records));
  for (const record of records) { fireEvent.click(row('RequestBoundary', record.id)); expect(row('RequestBoundary', record.id).getAttribute('aria-selected')).toBe('true'); }
  expect(screen.queryByText('Fold Steps')).toBeNull();
});

it('T1-06/10 visible Step segments preserve native order and migrate by owner after prepend', () => {
  const middle = traceRecord(10);
  const page = trajectoryItems([middle]);
  const segment = page.find((item): item is OwnedDisplayItem => item.type === 'StepHeader')!;
  const all = trajectoryItems([traceRecord(9), middle]);
  const target = preferredItem(all, middle.id, selectionOf(segment))!;
  expect(target.type).toBe('RequestBoundary'); expect(target.owner_record_id).toBe(middle.id);
  const split = trajectoryItems([traceRecord(9), traceRecord(11, { kind: 'background', request: null, location: {} }), middle]);
  expect(split.filter(item => item.type === 'StepHeader')).toHaveLength(2);
  expect(split.filter(item => item.type === 'RecordRow' || item.type === 'RequestBoundary').map(item => item.owner_record_id)).toEqual(['trace:9', 'trace:11', 'trace:10']);
});

const proposal = () => traceRecord(0, { kind: 'assistant', request: null, message_id: 'assistant-0', calls: [{ call_id: 'same', tool_id: 'tool-a', name: 'same name' }, { call_id: 'not-executed', tool_id: 'tool-a', name: 'same name' }] });
const execution = (n: number, overrides: Partial<TraceRecord> = {}) => traceTool(n, { tool: { ...traceTool(n).tool!, call_id: 'same', tool_id: 'tool-a', name: 'same name' }, ...overrides });
it('T1-07 exact scope isolates reused call IDs across Step/Attempt/Tool and page split', () => {
  const assistant = proposal();
  const unrelated = [execution(2, { location: { attempt_id: 'other', step_id: '1' } }), execution(3, { location: { attempt_id: 'attempt-a', step_id: '2' } }), execution(4, { tool: { ...execution(4).tool!, tool_id: 'tool-b' } }), execution(5, { location: {} })];
  expect(matchingCalls([assistant, execution(1), ...unrelated]).get(assistant.id)?.map(r => r.id)).toEqual(['trace:1']);
  const records = [assistant, execution(1), ...unrelated];
  const visible = visibleItems(trajectoryItems(records), records, new Set(), new Set([assistant.id]), null);
  const summary = visible.find(item => item.type === 'CollapsedCallSummary')!;
  expect(summary.preview).toContain('2 proposed · 1 loaded matching executions');
  expect(visible.filter(item => item.type === 'RecordRow').map(item => item.record.id)).toEqual(['trace:0', 'trace:2', 'trace:3', 'trace:4', 'trace:5']);
  expect(matchingCalls([execution(1)]).size).toBe(0);
  expect(matchingCalls([assistant]).size).toBe(0);
  expect(matchingCalls([assistant, { ...assistant, id: 'other-proposer' }, execution(1)]).size).toBe(0);
});

it('T1-08 Calls summary exposes warnings and leaves native domains independent', () => {
  const assistant = proposal();
  const executions = ['failed', 'denied', 'waiting', 'outcome_unknown'].map((state, n) => execution(n + 1, { state: state as TraceRecord['state'] }));
  const domains = ['background', 'subagent', 'workflow'].map((kind, n) => traceRecord(n + 10, { kind: kind as TraceRecord['kind'], request: null, originating_tool_call_id: 'same' }));
  const records = [assistant, ...executions, ...domains, traceRecord(20, { kind: 'compaction', request: null, state: 'running' })];
  const visible = visibleItems(trajectoryItems(records), records, new Set(), new Set([assistant.id]), null);
  const summary = visible.find(item => item.type === 'CollapsedCallSummary')!;
  for (const state of ['failed', 'denied', 'waiting', 'outcome_unknown']) expect(summary.preview).toContain(`1 ${state}`);
  for (const domain of domains) expect(visible.some(item => item.type === 'RecordRow' && item.record.id === domain.id)).toBe(true);
  expect(visible.find((item): item is OwnedDisplayItem => item.type === 'RecordRow' && item.record.id === 'trace:20')?.label).toBe('Compacting…');
  for (const state of ['incomplete', 'failed', 'completed'] as const) {
    const item = trajectoryItems([traceRecord(21, { kind: 'compaction', request: null, state })]).find(item => item.type === 'RecordRow')!;
    expect(item.label).toBe(state === 'completed' ? 'COMPACTED' : `Compaction · ${state}`);
  }
});

it('T1-09 search reveals both collapsed kinds without any detail/history reads', () => {
  const records = [proposal(), execution(1)]; const load = vi.fn(); const older = vi.fn();
  show(cacheOf(records), load, older);
  fireEvent.click(within(screen.getByRole('toolbar')).getByRole('button', { name: 'Collapse Calls' }));
  fireEvent.click(screen.getByRole('button', { name: 'Fold Attempts' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Search loaded Trace' }), { target: { value: 'ls -la' } });
  expect(row('RecordRow', 'trace:1')).not.toBeNull(); expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
  const items = trajectoryItems([richRequest()]);
  expect(searchItems(items, 'Preview context-a')?.size).toBe(1);
  expect(searchItems(items, 'Frozen prompt')?.size).toBeGreaterThan(0);
  expect(preferredItem(items, 'trace:0')?.type).toBe('RequestBoundary');
});

it('T1-10 512 native records plus synthetic items keep mounted rows and reads bounded', () => {
  const records = Array.from({ length: 512 }, (_, n) => richRequest(n));
  const items = trajectoryItems(records);
  expect(items.length).toBeGreaterThan(2000);
  expect(new Set(items.map(item => item.display_key)).size).toBe(items.length);
  const load = vi.fn(); show(cacheOf(records), load);
  expect(document.querySelectorAll('[data-display-key]').length).toBeLessThan(60);
  expect(load).not.toHaveBeenCalled();
});

it('T1-11 content-only updates never move an off-tail reader', () => {
  const records = Array.from({ length: 20 }, (_, n) => traceRecord(n)); const view = show(cacheOf(records));
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  Object.defineProperty(ledger, 'scrollHeight', { configurable: true, value: 900 });
  ledger.scrollTop = 150; fireEvent.scroll(ledger);
  const updated = records.map(record => ({ ...record, state: 'running' as const }));
  view.rerender(<Trajectory cache={cacheOf(updated)} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(ledger.scrollTop).toBe(150); expect(screen.getByRole('button', { name: 'Jump to latest' })).toBeDefined();
});

it('safe existing Tool renderers expose supported Code, Input, Result and Native', () => {
  const record = traceTool(1); show(completeTraceDetail(cacheOf([record]), record.id, 1, toolDetail(1)));
  fireEvent.click(row('RecordRow', record.id));
  for (const facet of ['Code', 'Input', 'Result', 'Native']) { fireEvent.click(screen.getByRole('tab', { name: facet })); expect(screen.getByRole('tab', { name: facet }).getAttribute('aria-selected')).toBe('true'); }
  expect(within(screen.getByRole('tabpanel')).getByText('tool-bash')).toBeDefined();
});


it('T1-12 sequence keeps equal glyph widths even when native duration is missing', () => {
  const records = [traceRecord(0, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } }), traceRecord(1)];
  show(cacheOf(records));
  const glyph = document.querySelector<HTMLElement>('[data-record-id="trace:0"]')!;
  expect(glyph.hasAttribute('data-marker')).toBe(false);
  expect(glyph.style.width).toBe('50%');
  fireEvent.click(screen.getByRole('button', { name: 'Duration' }));
  expect(glyph.getAttribute('data-marker')).toBe('true');
});
