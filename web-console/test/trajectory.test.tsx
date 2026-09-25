/* Copyright (c) 2026 DeepSeek. MIT. Adapted interaction contracts; see PROVENANCE.md. */
import { useCallback, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { beginTraceDetail, completeTraceDetail, replaceTrace, selectTrace, type TraceCache } from '../src/client/trace';
import { trajectoryItems, visibleItems, matchingCalls, preferredItem, preferredStructure, systemLabel, type InspectableDisplayItem } from '../src/app/trajectory/layout';
import { searchItems } from '../src/app/trajectory/search';
import { requestDetail, toolDetail, traceRecord, traceTool } from './trace-fixture';
import type { TraceContextPresentation, TraceDetail, TraceRecord } from '../../protocol/app-server/v24';

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

it('T1-06/10 segment anchors regroup within the same structure without acquiring detail ownership', () => {
  const middle = traceRecord(10);
  const segment = trajectoryItems([middle]).find(item => item.type === 'StepHeader')!;
  expect(segment.segment_anchor_record_id).toBe(middle.id);
  expect('owner_record_id' in segment).toBe(false);
  const nativeStep = traceRecord(8, { kind: 'step', request: null });
  const all = trajectoryItems([nativeStep, traceRecord(9), middle]);
  const target = preferredStructure(all, segment)!;
  expect(target.type).toBe('StepHeader');
  expect(target.native_record?.id).toBe(nativeStep.id);
  expect(target.segment_anchor_record_id).toBe(nativeStep.id);
  expect('owner_record_id' in target).toBe(false);
  const split = trajectoryItems([traceRecord(9), traceRecord(11, { kind: 'background', request: null, location: {} }), middle]);
  expect(split.filter(item => item.type === 'StepHeader')).toHaveLength(2);
  expect(preferredStructure(split, segment)?.segment_anchor_record_id).toBe(middle.id);
  expect(preferredStructure(trajectoryItems([traceRecord(9)]), segment)).toBeUndefined();
  expect(split.filter(item => item.type === 'RecordRow' || item.type === 'RequestBoundary').map(item => item.owner_record_id)).toEqual(['trace:9', 'trace:11', 'trace:10']);
});

it.each(['request', 'tool'] as const)('T1-04/06 mid-Step %s anchor never owns structural inspection', kind => {
  const child = kind === 'request' ? traceRecord(10) : traceTool(10);
  const load = vi.fn(); const select = vi.fn(); const older = vi.fn();
  render(<Trajectory cache={cacheOf([child])} onSelect={select} onLoadDetail={load} loadEarlier={older} latest={noop} />);
  const items = trajectoryItems([child]);
  const step = items.find(item => item.type === 'StepHeader')!;
  expect(step.display_key).toBe(JSON.stringify(['step-segment', 'attempt-a', '1', child.id]));
  expect(trajectoryItems([{ ...child, state: 'running' }]).find(item => item.type === 'StepHeader')!.display_key).toBe(step.display_key);
  for (const type of ['AttemptSectionHeader', 'StepHeader']) {
    const header = document.querySelector<HTMLElement>(`[data-display-type="${type}"]`)!;
    act(() => header.focus()); fireEvent.click(header); fireEvent.keyDown(header, { key: 'Enter' });
    expect(header.hasAttribute('data-owner')).toBe(false);
    expect(document.activeElement).toBe(header);
    expect(screen.queryByRole('complementary')).toBeNull();
  }
  expect(select.mock.calls.every(([id]) => id === undefined)).toBe(true);
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
  fireEvent.click(row(kind === 'request' ? 'RequestBoundary' : 'RecordRow', child.id));
  expect(select).toHaveBeenLastCalledWith(child.id);
  expect(load.mock.calls).toEqual([[child.id]]);
  expect(screen.getByRole('tab', { name: kind === 'request' ? 'System Prompt' : 'Input' })).toBeDefined();
});

it('T1-06 exact loaded native structures remain presentation-only and expose only their own evidence', () => {
  const attempt = traceRecord(8, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' }, state: 'running' });
  const step = traceRecord(9, { kind: 'step', request: null });
  const records = [attempt, step, traceTool(10)];
  const structures = trajectoryItems(records).filter(item => item.type === 'StepHeader' || item.type === 'AttemptSectionHeader');
  expect(structures.map(item => item.native_record?.id)).toEqual([attempt.id, step.id]);
  const load = vi.fn(); show(cacheOf(records), load);
  for (const item of structures) fireEvent.click(document.querySelector(`[data-display-type="${item.type}"]`)!);
  expect(screen.queryByRole('complementary')).toBeNull(); expect(load).not.toHaveBeenCalled();
  expect(document.querySelector('[data-display-type="AttemptSectionHeader"]')?.textContent).toContain('running');
});

it.each(['request', 'tool'] as const)('T1-04 controlled late %s detail cannot hijack structural focus', async kind => {
  const child = kind === 'request' ? traceRecord(10) : traceTool(10);
  let resolve!: (detail: TraceDetail) => void;
  const reads: string[] = []; const selections: (string | undefined)[] = [];
  function Fixture() {
    const [cache, setCache] = useState(cacheOf([child]));
    const load = useCallback((id: string) => {
      reads.push(id); setCache(current => beginTraceDetail(current, id));
      void new Promise<TraceDetail>(done => { resolve = done; }).then(detail => setCache(current => completeTraceDetail(current, id, 1, detail)));
    }, []);
    return <Trajectory cache={cache} onSelect={id => { selections.push(id); setCache(current => selectTrace(current, id)); }} onLoadDetail={load} loadEarlier={noop} latest={noop} />;
  }
  render(<Fixture />);
  fireEvent.click(row(kind === 'request' ? 'RequestBoundary' : 'RecordRow', child.id));
  const step = document.querySelector<HTMLElement>('[data-display-type="StepHeader"]')!;
  act(() => step.focus()); fireEvent.keyDown(step, { key: 'Enter' });
  expect(selections.at(-1)).toBeUndefined();
  await act(async () => resolve(kind === 'request' ? requestDetail(10) : toolDetail(10)));
  expect(document.activeElement).toBe(step); expect(step.getAttribute('data-selected')).toBe('true');
  expect(screen.queryByRole('complementary')).toBeNull(); expect(reads).toEqual([child.id]);
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
  expect(visible.find((item): item is InspectableDisplayItem => item.type === 'RecordRow' && item.record.id === 'trace:20')?.label).toBe('Compacting…');
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

it('T1-12 Journal duration remains visible in Inspector when Model timing lacks a bridge', () => {
  const record = traceRecord(0, { timing: { started_at: '2026-09-15T00:00:00Z', ended_at: '2026-09-15T00:00:09Z', duration_ms: '9000' } });
  record.request!.generation = { ttft_ms: '320', generation_ms: '1280', terminal_ms: '1600', output_tokens_per_second: null, timeline: null };
  show(cacheOf([record]));
  fireEvent.click(row('RequestBoundary'));
  fireEvent.click(screen.getByRole('tab', { name: 'Timing' }));
  const facts = within(screen.getByRole('tabpanel'));
  expect(facts.getByText('Journal wall duration').nextElementSibling?.textContent).toBe('9.00 s');
  expect(facts.getByText('Two authoritative durable timestamps.')).toBeDefined();
  expect(record.timing.duration_ms).toBe('9000');
});

it.each(['result', 'definition', 'arguments', 'tool'] as const)('Tool facets explicitly disclose absent %s at the read cut', missing => {
  const record = traceTool(10, { state: missing === 'result' ? 'running' : 'completed' });
  if (missing === 'result') record.tool!.outcome = null;
  const detail = toolDetail(10);
  if (missing === 'tool') { detail.tool = null; detail.truncated = true; }
  else { detail.tool![missing] = null; detail.tool!.source = null; }
  if (missing === 'result') detail.tool!.lifecycle = 'started';
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(row('RecordRow', record.id));
  expect(screen.queryByRole('tab', { name: 'Code' })).toBeNull();
  const expected = {
    Input: 'The canonical proposal for this call is not loadable at this read cut.',
    Result: 'No canonical Tool result is recorded at this read cut.',
    Schema: 'The historical Tool definition is unavailable at this read cut.',
  };
  for (const facet of ['Input', 'Result', 'Schema'] as const) {
    fireEvent.click(screen.getByRole('tab', { name: facet }));
    const panel = screen.getByRole('tabpanel');
    expect(panel.textContent?.trim().length).toBeGreaterThan(0);
    if (missing === 'tool') expect(within(panel).getByText('Tool detail is unavailable in this bounded detail projection.')).toBeDefined();
    else if ((facet === 'Input' && missing === 'arguments') || (facet === 'Result' && missing === 'result') || (facet === 'Schema' && missing === 'definition')) expect(within(panel).getByText(expected[facet])).toBeDefined();
  }
});

it('T1-09 structural search and Attempt collapse never borrow child facts or another Attempt', () => {
  const child = traceRecord(10, { preview: { text: 'unique child preview', truncated: false } });
  const other = traceTool(11, { location: { attempt_id: 'attempt-b', step_id: '1' } });
  const items = trajectoryItems([child, other]);
  const matches = searchItems(items, 'unique child preview')!;
  expect(items.filter(item => matches.has(item.display_key)).every(item => item.type !== 'StepHeader' && item.type !== 'AttemptSectionHeader')).toBe(true);
  const structural = items.find(item => item.type === 'StepHeader')!;
  expect(searchItems(items, 'Step 1')?.has(structural.display_key)).toBe(true);
  expect(preferredStructure(trajectoryItems([{ ...child, location: { attempt_id: 'attempt-b', step_id: '1' } }]), structural)).toBeUndefined();
  const visible = visibleItems(items, [child, other], new Set(['attempt-a']), new Set(), null);
  expect(visible.filter(item => item.type === 'RequestBoundary')).toHaveLength(0);
  expect(visible.filter(item => item.type === 'RecordRow').map(item => item.owner_record_id)).toEqual([other.id]);
  expect(visible.filter(item => item.type === 'StepHeader').map(item => item.attempt_id)).toEqual(['attempt-b']);
});

it('T1-04 timeline navigation explicitly selects its native Request after structural focus', () => {
  const child = traceRecord(10); const load = vi.fn(); show(cacheOf([child]), load);
  const header = document.querySelector<HTMLElement>('[data-display-type="StepHeader"]')!;
  act(() => header.focus());
  expect(load).not.toHaveBeenCalled();
  fireEvent.click(document.querySelector('[data-record-id="trace:10"]')!);
  expect(load.mock.calls).toEqual([[child.id]]);
  expect(row('RequestBoundary', child.id).getAttribute('data-selected')).toBe('true');
  expect(header.hasAttribute('data-selected')).toBe(false);
});

/** All reads are explicitly completed or rejected by the test, never a timer. */
function controlledDetails(records: TraceRecord[]) {
  const reads: string[] = []; const selections: (string | undefined)[] = [];
  const pending = new Map<string, { resolve: (detail: TraceDetail) => void; reject: (error: Error) => void }>();
  const older = vi.fn();
  function Fixture() {
    const [cache, setCache] = useState(cacheOf(records));
    const load = useCallback((id: string) => {
      reads.push(id); setCache(current => beginTraceDetail(current, id));
      void new Promise<TraceDetail>((resolve, reject) => pending.set(id, { resolve, reject })).then(
        detail => setCache(current => completeTraceDetail(current, id, 1, detail)),
        (error: Error) => setCache(current => completeTraceDetail(current, id, 1, undefined, error.message)),
      );
    }, []);
    return <Trajectory cache={cache} onSelect={id => { selections.push(id); setCache(current => selectTrace(current, id)); }} onLoadDetail={load} loadEarlier={older} latest={noop} />;
  }
  render(<Fixture />);
  return { reads, selections, pending, older };
}
const assistantDetail = () => requestDetail(0, { kind: 'assistant', request: null, messages: [] });
function collapseAndSelectSummary() {
  fireEvent.click(within(screen.getByRole('toolbar')).getByRole('button', { name: 'Collapse Calls' }));
  const summary = row('CollapsedCallSummary');
  expect(summary).not.toBeNull();
  act(() => summary.focus()); fireEvent.keyDown(summary, { key: 'Enter' });
  return summary;
}

it('T1-04 Calls summary retains display focus through delayed detail and falls back only on expansion', async () => {
  const { reads, selections, pending } = controlledDetails([proposal(), execution(1)]);
  const summary = collapseAndSelectSummary();
  const key = summary.dataset.displayKey;
  const assertSummary = () => {
    expect(summary.dataset.displayKey).toBe(key);
    expect(summary.getAttribute('data-selected')).toBe('true');
    expect(summary.getAttribute('aria-selected')).toBe('true');
    expect(row('RecordRow').getAttribute('aria-selected')).toBe('false');
    expect(document.activeElement).toBe(summary);
    expect(selections).toEqual(['trace:0']);
    expect(reads).toEqual(['trace:0']);
  };
  assertSummary();
  await act(async () => pending.get('trace:0')!.resolve(assistantDetail()));
  assertSummary();
  // Fire the explicit expansion while DOM focus is still on the summary.
  fireEvent.click(within(summary).getByRole('button', { name: 'Expand Calls' }));
  expect(row('CollapsedCallSummary')).toBeNull();
  expect(row('RecordRow').getAttribute('data-selected')).toBe('true');
  expect(document.activeElement).toBe(row('RecordRow'));
  expect(reads).toEqual(['trace:0']);
});

it.each(['expand', 'search', 'other owner'] as const)('T1-04 pending summary detail respects newer %s display state', async transition => {
  const { reads, selections, pending, older } = controlledDetails([proposal(), execution(1), traceRecord(2)]);
  const summary = collapseAndSelectSummary();
  if (transition === 'expand') fireEvent.click(within(summary).getByRole('button', { name: 'Expand Calls' }));
  else if (transition === 'search') {
    const search = screen.getByRole('textbox', { name: 'Search loaded Trace' });
    act(() => search.focus());
    fireEvent.change(search, { target: { value: 'ASSISTANT' } });
  } else {
    fireEvent.click(row('RequestBoundary', 'trace:2'));
    fireEvent.click(screen.getByRole('button', { name: 'View System Prompt' }));
    act(() => row('RequestBoundary', 'trace:2').focus());
  }
  const focus = document.activeElement;
  await act(async () => pending.get('trace:0')!.resolve(assistantDetail()));
  expect(document.activeElement).toBe(focus);
  expect(older).not.toHaveBeenCalled();
  if (transition === 'other owner') {
    expect(row('CollapsedCallSummary').getAttribute('aria-selected')).toBe('false');
    expect(row('RequestBoundary', 'trace:2').getAttribute('aria-selected')).toBe('true');
    expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
    expect(selections).toEqual(['trace:0', 'trace:2']);
    expect(reads).toEqual(['trace:0', 'trace:2']);
  } else {
    expect(row('CollapsedCallSummary')).toBeNull();
    expect(row('RecordRow').getAttribute('aria-selected')).toBe('true');
    expect(reads).toEqual(['trace:0']);
    if (transition === 'search') {
      fireEvent.change(screen.getByRole('textbox', { name: 'Search loaded Trace' }), { target: { value: '' } });
      expect(row('CollapsedCallSummary').getAttribute('aria-selected')).toBe('false');
      expect(row('RecordRow').getAttribute('aria-selected')).toBe('true');
    }
  }
});

it.each(['resolve', 'reject'] as const)('Tool facets distinguish pending historical reads from %s outcomes', async outcome => {
  const { reads, pending } = controlledDetails([traceTool(10)]);
  fireEvent.click(row('RecordRow', 'trace:10'));
  const facets = ['Input', 'Result', 'Schema'] as const;
  const noAbsence = (panel: HTMLElement) => {
    expect(within(panel).queryByText(/bounded detail projection|No canonical Tool result|canonical proposal.*not loadable|historical Tool definition is unavailable/)).toBeNull();
  };
  for (const facet of facets) {
    fireEvent.click(screen.getByRole('tab', { name: facet }));
    const panel = screen.getByRole('tabpanel');
    expect(within(panel).getByRole('status').textContent).toBe('Loading record detail…');
    noAbsence(panel); expect(reads).toEqual(['trace:10']);
    expect(screen.queryByRole('tab', { name: 'Code' })).toBeNull();
  }
  const detail = toolDetail(10, { tool: null, truncated: true });
  await act(async () => {
    if (outcome === 'resolve') pending.get('trace:10')!.resolve(detail);
    else pending.get('trace:10')!.reject(new Error('Controlled historical read failure'));
  });
  for (const facet of facets) {
    fireEvent.click(screen.getByRole('tab', { name: facet }));
    const panel = screen.getByRole('tabpanel');
    expect(within(panel).queryByRole('status')).toBeNull();
    if (outcome === 'resolve') expect(within(panel).getByText('Tool detail is unavailable in this bounded detail projection.')).toBeDefined();
    else {
      expect(within(panel).getByRole('alert').textContent).toBe(`${facet} could not be established because the historical detail read failed: Controlled historical read failure`);
      noAbsence(panel);
    }
    expect(reads).toEqual(['trace:10']);
  }
});

it('Agent activations retain separate trace records correlated to one durable Agent across replay', () => {
  const records = ['activation-a', 'activation-b'].map((activation_id, n) => traceRecord(n, {
    kind: 'subagent', request: null, agent_id: 'durable-agent', activation_id, native_id: activation_id,
  }));
  const ui = show(cacheOf(records));
  for (const record of records) {
    fireEvent.click(row('RecordRow', record.id));
    fireEvent.click(screen.getByRole('tab', { name: 'Native' }));
    const panel = within(screen.getByRole('tabpanel'));
    expect(panel.getByText('durable-agent')).toBeTruthy();
    expect(panel.getAllByText(record.activation_id!)).not.toHaveLength(0);
  }
  ui.rerender(<Trajectory cache={cacheOf(structuredClone(records))} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop}/>);
  expect(document.querySelectorAll('[data-display-type="RecordRow"]')).toHaveLength(2);
});
