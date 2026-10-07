import { traceStateLabel } from '../src/bindings/status-labels';
import { localeController } from '../src/locale/controller';
import { translator } from '../src/locale/translation';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted interaction contracts; see PROVENANCE.md. */
import { timelineFocus, timelineProjectionRevision, trajectoryTimeline } from '../src/app/trajectory/timeline';
import { useCallback, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { prependTrace, beginTraceDetail, completeTraceDetail, refreshTrace, replaceTrace, selectTrace, type TraceCache } from '../src/client/trace';
import { ledgerFocusTargets, ledgerRows, matchedRecordIds, isInspectable, projectTrajectory, trajectoryItems as flattenTrajectory, visibleItems, matchingCalls, preferredItem, systemPresentation, type InspectableDisplayItem, type TurnStructure } from '../src/app/trajectory/layout';
import { searchItems } from '../src/app/trajectory/search';
import { stepLessRecords, manyStepRecords, orderedStepRecords, structuralSearchRecords, requestDetail, toolDetail, traceRecord, traceTool } from './trace-fixture';
import type { TraceContextPresentation, TraceDetail, TraceRecord } from '../../protocol/app-server/v36';

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(360);
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value: function (this: HTMLElement, options: ScrollToOptions) { this.scrollTop = options.top ?? 0; } });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const noop = () => {};
const trajectoryItems = (records: readonly TraceRecord[]) => flattenTrajectory(translator('en'), projectTrajectory(translator('en'), records));
const cacheOf = (records: TraceRecord[]) => replaceTrace({ records, next_cursor: null });
function show(cache: TraceCache, load = vi.fn(), older = vi.fn()) {
  return render(<Trajectory cache={cache} loadEarlier={older} onSelect={noop} onLoadDetail={load} />);
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
function row(type: string, id = 'trace:0') { return document.querySelector<HTMLElement>(type === 'RequestBoundary' ? `[data-request-owner="${id}"]` : `[data-display-type="${type}"][data-owner="${id}"]`)!; }
/** The current Timeline canvas, measured as a fixed 100px so clientX is a percent. */
function timelineCanvas() {
  const canvas = screen.getByLabelText('Timeline overview; drag horizontally to focus events');
  canvas.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, right: 100, bottom: 40, width: 100, height: 40, toJSON: () => ({}) });
  canvas.setPointerCapture = () => {};
  return canvas;
}
/** Drag a Timeline interval in percent of the canvas. */
function dragTimeline(from: number, to: number) {
  const canvas = timelineCanvas();
  fireEvent.pointerDown(canvas, { button: 0, pointerId: 1, clientX: from });
  fireEvent.pointerMove(canvas, { pointerId: 1, clientX: to });
  fireEvent.pointerUp(canvas, { pointerId: 1, clientX: to });
}
/** Every rendered inspectable row's Timeline focus membership, by native owner. */
const timelineFocusOf = () => Object.fromEntries([...document.querySelectorAll<HTMLElement>('[data-owner]')].map(el => [el.dataset.owner, el.dataset.timelineFocus]));
const focusOverlay = () => document.querySelector<HTMLElement>('[data-focus-range]');
const overlayLeft = () => focusOverlay()!.style.getPropertyValue('--trajectory-selection-left');
const overlayWidth = () => focusOverlay()!.style.getPropertyValue('--trajectory-selection-width');
/** Wheel over the overview, as Harness zooms its domain. */
const zoomTimeline = () => { timelineCanvas(); fireEvent.wheel(screen.getByRole('region', { name: 'Trajectory timeline' }), { deltaY: -500, clientX: 50 }); };
/** The ledger row that opens one Turn, by its label. */
const turnRow = (label: string) => screen.getByLabelText(label, { selector: 'span' }).closest<HTMLElement>('[role="row"]')!;
/** Fold a Turn the Harness way: double-click its opening row. */
const foldTurn = (label: string) => fireEvent.doubleClick(turnRow(label));
/** A folded Turn's summary row, which expands it on click. */
const turnSummary = (attempt: string) => document.querySelector<HTMLElement>(`[data-display-type="TurnSummary"][data-attempt="${attempt}"]`);
/** Press and release one overview block, as a pointer click selects it. */
function pressSpan(id: string) {
  timelineCanvas();
  const span = [...document.querySelectorAll<HTMLElement>('[data-record-id]')].find(node => node.dataset.recordId === id)!;
  fireEvent.pointerDown(span, { button: 0, pointerId: 7, clientX: 10 });
  fireEvent.pointerUp(span, { pointerId: 7, clientX: 10 });
}

// v26 exposes two independent enums: cover their full Cartesian product,
// including combinations today's all-or-nothing snapshot producer cannot emit.
const inputMatrix = [
  ['initial', 'initial', 'Initial System Prompt', 'System Prompt'],
  ['initial', 'changed', 'Initial System Prompt · Tools Updated', 'System Prompt'],
  ['initial', 'unchanged', 'Initial System Prompt', 'System Prompt'],
  ['initial', 'previous_unavailable', 'Initial System Prompt · Previous Tool catalog unavailable', 'System Prompt'],
  ['changed', 'initial', 'System Prompt Updated · Initial Tools', 'Diff'],
  ['changed', 'changed', 'System Prompt and Tools Updated', 'Diff'],
  ['changed', 'unchanged', 'System Prompt Updated', 'Diff'],
  ['changed', 'previous_unavailable', 'System Prompt Updated · Previous Tool catalog unavailable', 'Diff'],
  ['unchanged', 'initial', 'Initial Tools', 'Tools'],
  ['unchanged', 'changed', 'Tools Updated', 'Tools'],
  ['unchanged', 'unchanged', undefined, 'Summary'],
  ['unchanged', 'previous_unavailable', 'Previous Tool catalog unavailable', 'System Prompt'],
  ['previous_unavailable', 'initial', 'Previous System Prompt unavailable · Initial Tools', 'Tools'],
  ['previous_unavailable', 'changed', 'Previous System Prompt unavailable · Tools Updated', 'Tools'],
  ['previous_unavailable', 'unchanged', 'Previous System Prompt unavailable', 'System Prompt'],
  ['previous_unavailable', 'previous_unavailable', 'Previous System Prompt unavailable · Previous Tool catalog unavailable', 'System Prompt'],
] as const;

it.each(inputMatrix)('T1-01 preserves native %s + %s without classification reads', (prompt, tools, label, facet) => {
  const record = richRequest();
  record.request!.system_prompt = { state: prompt, preview: { text: 'Identical bounded preview', truncated: true } };
  record.request!.tool_catalog = tools;
  const ids = { 'System Prompt': 'system-prompt', Diff: 'diff', Tools: 'tools', Summary: 'overview' } as const;
  expect(systemPresentation(translator('en'), record)).toEqual(label ? { label, facet: ids[facet] } : undefined);
  const cells = trajectoryItems([record]).filter(item => item.type === 'SystemPromptCell');
  expect(cells).toHaveLength(label ? 1 : 0);
  const load = vi.fn();
  show(cacheOf([record]), load);
  expect(load).not.toHaveBeenCalled();
  if (label) {
    expect(row('SystemPromptCell').textContent).toContain(label);
    expect(row('SystemPromptCell').querySelector('[data-system-prompt-state]')?.getAttribute('data-system-prompt-state')).toBe(prompt);
    expect(row('SystemPromptCell').querySelector('[data-tool-catalog-state]')?.getAttribute('data-tool-catalog-state')).toBe(tools);
  }
  fireEvent.click(row(label ? 'SystemPromptCell' : 'RequestBoundary'));
  expect(screen.getByRole('tab', { name: facet }).getAttribute('aria-selected')).toBe('true');
  expect(screen.queryByRole('tab', { name: 'Diff' }) !== null).toBe(prompt === 'changed');
  expect(Boolean(screen.queryByRole('tab', { name: 'Tools' }))).toBe(Boolean(label));
  expect(load.mock.calls).toEqual([[record.id]]); // only the selected immutable owner
  fireEvent.click(row('RequestBoundary'));
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Summary', 'Options', 'Usage', 'Timing']);
});

it.each([
  ['changed', 'previous_unavailable', 'Diff'],
  ['previous_unavailable', 'changed', 'Tools'],
] as const)('mixed %s + %s retains exact detail and independent facets', (prompt, tools, facet) => {
  const record = richRequest();
  record.request!.system_prompt.state = prompt;
  record.request!.tool_catalog = tools;
  const detail = requestDetail(0);
  if (prompt === 'previous_unavailable') {
    detail.request!.previous_system_prompt = null;
    detail.request!.predecessor = { availability: 'unavailable', request_id: 'previous-request' };
  }
  const load = vi.fn();
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail), load);
  fireEvent.click(row('SystemPromptCell'));
  expect(screen.getByRole('tab', { name: facet }).getAttribute('aria-selected')).toBe('true');
  if (prompt === 'changed') {
    expect(screen.getByRole('tabpanel').textContent).toContain('-Previous prompt.');
    expect(screen.getByRole('tabpanel').textContent).toContain('+You are the historical agent.');
  } else {
    expect(screen.queryByRole('tab', { name: 'Diff' })).toBeNull();
  }
  fireEvent.click(screen.getByRole('tab', { name: 'Tools' }));
  expect(screen.getByRole('tabpanel').textContent).toContain('Run one command.');
  expect(load).not.toHaveBeenCalled();
});

it('T1-04 preserves frozen Context order and exact owner/display/facet identities', () => {
  const record = richRequest();
  const items = trajectoryItems([record]);
  expect(items.map(item => item.type)).toEqual(['TurnHeader', 'GroupHeader', 'SystemPromptCell', 'ContextRow', 'ContextRow', 'RequestBoundary']);
  const contexts = items.filter(item => item.type === 'ContextRow');
  expect(contexts.map(item => item.context_message_id)).toEqual(['context-z', 'context-a']);
  expect(contexts.map(item => item.owner_record_id)).toEqual(['trace:0', 'trace:0']);
  expect(new Set(items.map(item => item.display_key)).size).toBe(items.length);
  const load = vi.fn(); show(cacheOf([record]), load);
  fireEvent.click(row('SystemPromptCell')); expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
  fireEvent.click(document.querySelector('[data-display-type="ContextRow"][data-display-key*="context-a"]')!);
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
  expect(screen.getByRole('tabpanel').textContent).toContain('Preview context-a');
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
    return <Trajectory cache={cache} onSelect={id => setCache(current => selectTrace(current, id))} onLoadDetail={load} loadEarlier={noop} />;
  }
  render(<Fixture />);
  fireEvent.click(row('SystemPromptCell'));
  fireEvent.click(row('RequestBoundary', 'trace:1'));
  fireEvent.click(row('ContextRow', 'trace:1'));
  const selected = row('ContextRow', 'trace:1'); selected.focus();
  expect(reads).toEqual(['trace:0', 'trace:1']);
  await act(async () => { resolvers.get('trace:1')!(requestDetail(1)); });
  await act(async () => { resolvers.get('trace:0')!(requestDetail(0)); });
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
  expect(selected.getAttribute('aria-selected')).toBe('true');
  expect(document.activeElement).toBe(selected);
  fireEvent.click(row('SystemPromptCell', 'trace:1'));
  fireEvent.click(row('RequestBoundary', 'trace:1'));
  expect(reads).toEqual(['trace:0', 'trace:1']);
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
});

it('T1-05 latest unchanged-only page inspects request options with one lazy owner read', () => {
  const load = vi.fn(); show(cacheOf([traceRecord(9), traceRecord(10)]), load);
  expect(load).not.toHaveBeenCalled(); expect(document.querySelector('[data-display-type="SystemPromptCell"]')).toBeNull();
  fireEvent.click(row('RequestBoundary', 'trace:10'));
  fireEvent.click(screen.getByRole('tab', { name: 'Options' }));
  expect(screen.getByRole('tab', { name: 'Options' }).getAttribute('aria-selected')).toBe('true');
  expect(load.mock.calls).toEqual([['trace:10']]);
});

it.each([[false, true], [true, false], [true, true]])('T1-02 independent truncation previous=%s current=%s forbids complete diff', (previous, current) => {
  const record = richRequest(); record.request!.system_prompt.state = 'changed';
  const detail = requestDetail(0); detail.request!.previous_system_prompt!.truncated = previous; detail.request!.effective_system_prompt.truncated = current;
  detail.request!.previous_system_prompt!.text = detail.request!.effective_system_prompt.text;
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(row('SystemPromptCell')); fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
  expect(screen.getByText(/A complete diff cannot be produced:/)).toBeDefined();
  expect(screen.queryByText(/No changes/)).toBeNull();
});

it('T1-02 diff distinguishes initial, unavailable, empty and complete changed content', () => {
  const record = richRequest(); record.request!.system_prompt.state = 'changed';
  const detail = requestDetail(0); detail.request!.previous_system_prompt = { text: '', truncated: false }; detail.request!.effective_system_prompt = { text: 'new prompt\n', truncated: false };
  const cache = completeTraceDetail(cacheOf([record]), record.id, 1, detail);
  const view = show(cache); fireEvent.click(row('SystemPromptCell')); fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
  expect(screen.getByRole('tabpanel').textContent).toBe('System Prompt@@ -1,0 +1,1 @@\n+new prompt\n');
  const update = (availability: 'not_applicable' | 'unavailable') => {
    const next = requestDetail(0); next.request!.previous_system_prompt = null;
    next.request!.predecessor = availability === 'not_applicable' ? { availability } : { availability, request_id: 'previous' };
    view.rerender(<Trajectory cache={completeTraceDetail(cache, record.id, 1, next)} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  };
  update('not_applicable'); expect(screen.getByText('No predecessor · initial prompt.')).toBeDefined();
  update('unavailable'); expect(screen.getByText(/Previous prompt unavailable/)).toBeDefined();
});

it('T1-06 retries stay one logical Step; failed and running requests need no Assistant', () => {
  const records = ['failed', 'failed', 'running', 'completed'].map((state, n) => traceRecord(n, { state: state as TraceRecord['state'] }));
  const items = trajectoryItems(records);
  expect(items.filter(item => item.type === 'GroupHeader')).toHaveLength(1);
  expect(items.filter(item => item.type === 'RequestBoundary').map(item => item.record.request!.retry_number)).toEqual([0, 1, 2, 3]);
  show(cacheOf(records));
  for (const record of records) { fireEvent.click(row('RequestBoundary', record.id)); expect(row('RequestBoundary', record.id).getAttribute('aria-pressed')).toBe('true'); }
  expect(screen.queryByText('Fold Steps')).toBeNull();
});

it('T1-06/10 Turn anchors regroup within the same Attempt without acquiring detail ownership', () => {
  const middle = traceRecord(10);
  const turn = trajectoryItems([middle]).find((item): item is TurnStructure => item.type === 'TurnHeader')!;
  expect(turn.anchor_record_id).toBe(middle.id);
  expect('owner_record_id' in turn).toBe(false);
  const nativeAttempt = traceRecord(8, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' } });
  const all = trajectoryItems([nativeAttempt, traceRecord(9), middle]);
  const target = all.find((item): item is TurnStructure => item.type === 'TurnHeader')!;
  expect(target.native_record?.id).toBe(nativeAttempt.id);
  expect(target.anchor_record_id).toBe(nativeAttempt.id);
  expect('owner_record_id' in target).toBe(false);
  const split = trajectoryItems([traceRecord(9), traceRecord(11, { kind: 'background', request: null, location: {} }), middle]);
  expect(split.filter(item => item.type === 'GroupHeader')).toHaveLength(1);
  expect(split.filter(item => item.type === 'RecordRow' || item.type === 'RequestBoundary').map(item => item.owner_record_id)).toEqual(['trace:9', 'trace:10', 'trace:11']);
});

it.each(['request', 'tool'] as const)('T1-04/06 mid-Turn %s anchor gives the Turn a label, never a selectable structure', kind => {
  const child = kind === 'request' ? traceRecord(10) : traceTool(10);
  const load = vi.fn(); const select = vi.fn(); const older = vi.fn();
  render(<Trajectory cache={cacheOf([child])} onSelect={select} onLoadDetail={load} loadEarlier={older} />);
  const step = trajectoryItems([child]).find(item => item.type === 'GroupHeader')!;
  expect(step.display_key).toBe(JSON.stringify(['group', 'attempt-a', '1']));
  expect(trajectoryItems([{ ...child, state: 'running' }]).find(item => item.type === 'GroupHeader')!.display_key).toBe(step.display_key);
  // As in Harness, a Step has no chrome and the Turn label is not a control.
  expect(document.querySelector('[data-structural]')).toBeNull();
  const label = screen.getByLabelText('Turn 1', { selector: 'span' });
  expect(label.tagName).toBe('SPAN');
  fireEvent.click(label);
  expect(select.mock.calls.every(([id]) => id === child.id)).toBe(true);
  expect(older).not.toHaveBeenCalled();
  fireEvent.click(row(kind === 'request' ? 'RequestBoundary' : 'RecordRow', child.id));
  expect(select).toHaveBeenLastCalledWith(child.id);
  expect(load.mock.calls).toEqual([[child.id]]);
  expect(screen.getByRole('tab', { name: kind === 'request' ? 'Options' : 'Payload' })).toBeDefined();
});

const proposal = () => traceRecord(0, { kind: 'assistant', request: null, message_id: 'assistant-0', calls: [{ call_id: 'same', tool_id: 'tool-a', name: 'same name' }, { call_id: 'not-executed', tool_id: 'tool-a', name: 'same name' }] });
const execution = (n: number, overrides: Partial<TraceRecord> = {}) => traceTool(n, { tool: { ...traceTool(n).tool!, call_id: 'same', tool_id: 'tool-a', name: 'same name' }, ...overrides });
it('T1-07 exact scope isolates reused call IDs across Step/Attempt/Tool and page split', () => {
  const assistant = proposal();
  const unrelated = [execution(2, { location: { attempt_id: 'other', step_id: '1' } }), execution(3, { location: { attempt_id: 'attempt-a', step_id: '2' } }), execution(4, { tool: { ...execution(4).tool!, tool_id: 'tool-b' } }), execution(5, { location: {} })];
  expect(matchingCalls([assistant, execution(1), ...unrelated]).get(assistant.id)?.map(r => r.id)).toEqual(['trace:1']);
  const records = [assistant, execution(1), ...unrelated];
  const visible = visibleItems(translator('en'), trajectoryItems(records), records, new Set([assistant.id]), null);
  const summary = visible.find(item => item.type === 'CallsSummary')!;
  expect(summary.summary).toBe('1 tool call · same name');
  expect(visible.filter(item => item.type === 'RecordRow').map(item => item.record.id)).toEqual(['trace:0', 'trace:4', 'trace:3', 'trace:2', 'trace:5']);
  expect(matchingCalls([execution(1)]).size).toBe(0);
  expect(matchingCalls([assistant]).size).toBe(0);
  expect(matchingCalls([assistant, { ...assistant, id: 'other-proposer' }, execution(1)]).size).toBe(0);
});

it.each(['en', 'zh'] as const)('T1-08 Calls summary localizes in %s and leaves native domains independent', locale => {
  const tx = translator(locale);
  const assistant = proposal();
  const states = ['completed', 'failed', 'denied', 'waiting', 'outcome_unknown'] as const;
  const executions = states.map((state, n) => execution(n + 1, { state }));
  const domains = ['background', 'subagent', 'workflow'].map((kind, n) => traceRecord(n + 10, { kind: kind as TraceRecord['kind'], request: null, originating_tool_call_id: 'same' }));
  const records = [assistant, ...executions, ...domains, traceRecord(20, { kind: 'compaction', request: null, state: 'running' })];
  const visible = visibleItems(tx, flattenTrajectory(tx, projectTrajectory(tx, records)), records, new Set([assistant.id]), null);
  const summary = visible.find(item => item.type === 'CallsSummary')!;
  // Harness names the folded calls by count and Tool name, never by lifecycle.
  expect(summary.summary).toBe(locale === 'en' ? '5 tool calls · same name' : '5 个工具调用 · same name');
  expect(executions.map(record => record.state)).toEqual(states);
  for (const domain of domains) expect(visible.some(item => item.type === 'RecordRow' && item.record.id === domain.id)).toBe(true);
  expect(visible.find((item): item is InspectableDisplayItem => item.type === 'RecordRow' && item.record.id === 'trace:20')?.label).toBe(tx('trajectory:copy.compacting'));
  for (const state of ['incomplete', 'failed', 'completed'] as const) {
    const item = flattenTrajectory(tx, projectTrajectory(tx, [traceRecord(21, { kind: 'compaction', request: null, state })])).find(item => item.type === 'RecordRow')!;
    expect(item.label).toBe(state === 'completed' ? tx('trajectory:compacted') : tx('trajectory:copy.compaction-value', { p0: traceStateLabel(tx, state) }));
  }
});

it('T1-09 search reveals both collapsed kinds without any detail/history reads', () => {
  const records = [proposal(), execution(1)]; const load = vi.fn(); const older = vi.fn();
  show(cacheOf(records), load, older);
  fireEvent.click(within(screen.getByRole('toolbar')).getByRole('button', { name: 'Collapse calls' }));
  fireEvent.click(screen.getByRole('button', { name: 'Collapse turns' }));
  fireEvent.change(screen.getByRole('searchbox', { name: 'Search trajectory' }), { target: { value: 'ls -la' } });
  expect(row('RecordRow', 'trace:1')).not.toBeNull(); expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
  const items = trajectoryItems([richRequest()]);
  expect(searchItems(projectTrajectory(translator('en'), [richRequest()]), 'Preview context-a')?.size).toBe(1);
  expect(searchItems(projectTrajectory(translator('en'), [richRequest()]), 'Frozen prompt')?.size).toBe(1);
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
  view.rerender(<Trajectory cache={cacheOf(updated)} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(ledger.scrollTop).toBe(150);
});

it('safe existing Tool renderers expose supported Code, Result, Schema and Timing', () => {
  const record = traceTool(1); show(completeTraceDetail(cacheOf([record]), record.id, 1, toolDetail(1)));
  fireEvent.click(row('RecordRow', record.id));
  for (const facet of ['Code', 'Result', 'Schema', 'Timing']) { fireEvent.click(screen.getByRole('tab', { name: facet })); expect(screen.getByRole('tab', { name: facet }).getAttribute('aria-selected')).toBe('true'); }
  expect(within(screen.getByRole('tabpanel')).getByText('1,000 ms')).toBeDefined();
});


it('T1-12 sequence keeps equal glyph widths even when native duration is missing', () => {
  const records = [traceRecord(0, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } }), traceRecord(1)];
  show(cacheOf(records));
  const width = () => document.querySelector<HTMLElement>('[data-record-id="trace:0"]')!.style.getPropertyValue('--trajectory-span-width');
  expect(width()).toBe('50%');
  // A record with one endpoint is drawn at the 2px minimum, never a duration.
  fireEvent.click(screen.getByRole('button', { name: 'Use actual duration' }));
  expect(width()).toBe('0%');
});

it('T1-12 Journal duration remains visible in Inspector when Model timing lacks a bridge', () => {
  const record = traceRecord(0, { timing: { started_at: '2026-09-15T00:00:00Z', ended_at: '2026-09-15T00:00:09Z', duration_ms: '9000' } });
  record.request!.generation = { ttft_ms: '320', generation_ms: '1280', terminal_ms: '1600', output_tokens_per_second: null, timeline: null };
  show(cacheOf([record]));
  fireEvent.click(row('RequestBoundary'));
  fireEvent.click(screen.getByRole('tab', { name: 'Timing' }));
  const facts = within(screen.getByRole('tabpanel'));
  expect(facts.getByText('Total duration').nextElementSibling?.textContent).toBe('9.00 s');
  expect(facts.getByText('TTFT').nextElementSibling?.textContent).toBe('320 ms');
  expect(facts.getByText('Generation').nextElementSibling?.textContent).toBe('1.28 s');
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
  // Harness's own absence copy; a running call has no result yet.
  const expected = { Payload: 'No payload captured', Result: missing === 'result' ? 'Running…' : 'No result captured', Schema: 'Schema unavailable' };
  for (const facet of ['Payload', 'Result', 'Schema'] as const) {
    fireEvent.click(screen.getByRole('tab', { name: facet }));
    const panel = screen.getByRole('tabpanel');
    const absent = missing === 'tool' || (facet === 'Payload' && missing === 'arguments') || (facet === 'Result' && missing === 'result') || (facet === 'Schema' && missing === 'definition');
    if (absent) expect(within(panel).getByText(expected[facet])).toBeDefined();
    else expect(within(panel).queryByText(expected[facet])).toBeNull();
  }
});

it('T1-09 structural search and Attempt collapse never borrow child facts or another Attempt', () => {
  const child = traceRecord(10, { preview: { text: 'unique child preview', truncated: false } });
  const other = traceTool(11, { location: { attempt_id: 'attempt-b', step_id: '1' } });
  const items = trajectoryItems([child, other]);
  const matches = searchItems(projectTrajectory(translator('en'), [child, other]), 'unique child preview')!;
  expect(items.filter(item => matches.has(item.display_key)).every(item => item.type !== 'GroupHeader' && item.type !== 'TurnHeader')).toBe(true);
  const structural = items.find((item): item is TurnStructure => item.type === 'TurnHeader')!;
  expect(searchItems(projectTrajectory(translator('en'), [child, other]), 'Step 1')).toEqual(new Set(items.filter(isInspectable).map(item => item.display_key)));
  expect(structural.record_ids).toEqual([child.id]);
  // A Turn with a single content row has nothing to fold, as in Harness.
  const visible = visibleItems(translator('en'), items, [child, other], new Set(), null);
  const ledger = ledgerRows(translator('en'), projectTrajectory(translator('en'), [child, other]), visible, new Set(['attempt-a', 'attempt-b']), false);
  expect(ledger.some(row => row.kind === 'summary')).toBe(false);
  expect(ledger.flatMap(row => row.item ? [row.item.owner_record_id] : [])).toEqual([child.id, other.id]);
});

it('T1-04 timeline navigation explicitly selects its native Request', () => {
  const child = traceRecord(10); const load = vi.fn(); show(cacheOf([child]), load);
  expect(load).not.toHaveBeenCalled();
  pressSpan('trace:10');
  expect(load.mock.calls).toEqual([[child.id]]);
  expect(row('RequestBoundary', child.id).getAttribute('data-selected')).toBe('true');
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
    return <Trajectory cache={cache} onSelect={id => { selections.push(id); setCache(current => selectTrace(current, id)); }} onLoadDetail={load} loadEarlier={older} />;
  }
  render(<Fixture />);
  return { reads, selections, pending, older };
}
it.each(['resolve', 'reject'] as const)('Tool facets distinguish pending historical reads from %s outcomes', async outcome => {
  const { reads, pending } = controlledDetails([traceTool(10)]);
  fireEvent.click(row('RecordRow', 'trace:10'));
  const facets = ['Payload', 'Result', 'Schema'] as const;
  const noAbsence = (panel: HTMLElement) => {
    expect(within(panel).queryByText(/No payload captured|No result captured|Schema unavailable/)).toBeNull();
  };
  for (const facet of facets) {
    fireEvent.click(screen.getByRole('tab', { name: facet }));
    const panel = screen.getByRole('tabpanel');
    expect(within(panel).getByRole('status').textContent).toBe('Loading details…');
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
    if (outcome === 'resolve') expect(within(panel).getByText({ Payload: 'No payload captured', Result: 'No result captured', Schema: 'Schema unavailable' }[facet])).toBeDefined();
    else {
      expect(within(panel).getByRole('alert').textContent).toBe('Controlled historical read failure');
      noAbsence(panel);
    }
    expect(reads).toEqual(['trace:10']);
  }
});

it('407: exact Attempt and Step identity own one Turn/group across interleaved records and retries', () => {
  const records = [
    traceRecord(0, { kind: 'user', request: null, location: {}, preview: { text: 'outside input', truncated: false } }),
    traceRecord(1, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } }),
    traceRecord(2, { location: { attempt_id: 'attempt-a', step_id: 'opaque-step' } }),
    traceRecord(3, { location: { attempt_id: 'attempt-b', step_id: 'other-step' } }),
    traceRecord(4, { location: { attempt_id: 'attempt-a', step_id: 'opaque-step' }, state: 'failed' }),
    traceRecord(5, { location: { attempt_id: 'attempt-a', step_id: 'next-step' } }),
  ];
  const projection = projectTrajectory(translator('en'), records);
  expect(projection.sections.map(section => section.kind)).toEqual(['outside', 'turn', 'turn']);
  const turn = projection.sections[1]!;
  expect(turn.kind).toBe('turn');
  if (turn.kind !== 'turn') throw new Error('Expected Turn');
  expect(turn.groups.map(group => [group.kind, group.nativeStepId, group.label])).toEqual([
    ['message', undefined, 'Message'], ['step', 'opaque-step', 'Step 1'], ['step', 'next-step', 'Step 2'],
  ]);
  expect(turn.groups[1]!.cells.map(cell => cell.owner_record_id)).toEqual(['trace:2', 'trace:4']);
  const items = flattenTrajectory(translator('en'), projection);
  expect(items.filter(item => item.type === 'TurnHeader').map(item => item.label)).toEqual(['Turn 1', 'Turn 2']);
  expect(items.filter(item => item.type === 'GroupHeader').map(item => item.label)).toEqual(['Message', 'Step 1', 'Step 2', 'Step 1']);
  const folded = visibleItems(translator('en'), items, records, new Set(), null);
  // A Turn with one content row has nothing to fold, as in Harness.
  expect(ledgerRows(translator('en'), projection, folded, new Set(['attempt-a', 'attempt-b']), false).flatMap(row => row.item ? [row.item.owner_record_id] : [])).toEqual(['trace:0', 'trace:1', 'trace:2', 'trace:4', 'trace:5', 'trace:3']);
  expect(trajectoryTimeline(translator('en'), projection, 'sequence')!.boundaries.map(boundary => boundary.nativeAttemptId)).toEqual(
    items.filter(item => item.type === 'TurnHeader').map(item => item.attempt_id),
  );
  show(cacheOf(records));
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect(ledger.textContent).not.toContain('opaque-step');
  expect(ledger.textContent).not.toContain('attempt-a');
  fireEvent.click(row('RequestBoundary', 'trace:2'));
  expect(row('RequestBoundary', 'trace:2').getAttribute('aria-pressed')).toBe('true');
  expect(screen.queryByRole('tab', { name: 'Native' })).toBeNull();
});

it('407: prepend renumbers Turn while collapsed selection, prompt detail and focus retain native identity', () => {
  const selected = richRequest(10);
  const detail = requestDetail(10);
  detail.request!.effective_system_prompt.text = 'Exact frozen request ten';
  const initial = completeTraceDetail(cacheOf([selected]), selected.id, 1, detail);
  const older = traceRecord(1, { location: { attempt_id: 'older-attempt', step_id: 'older-step' } });
  const load = vi.fn();
  const view = show(initial, load);
  fireEvent.click(row('SystemPromptCell', selected.id));
  foldTurn('Turn 1');
  const summary = turnSummary('attempt-a')!;
  expect(summary).not.toBeNull();
  act(() => summary.focus());
  expect(screen.getByRole('complementary').textContent).toContain('Exact frozen request ten');
  expect(row('SystemPromptCell', selected.id)).not.toBeNull();
  const next = prependTrace(initial, { records: [older], next_cursor: null });
  view.rerender(<Trajectory cache={next} loadEarlier={noop} onSelect={noop} onLoadDetail={load} />);
  expect(turnSummary('attempt-a')).toBe(summary);
  expect(screen.getByLabelText('Turn 2', { selector: 'span' }).closest('[data-attempt]')?.getAttribute('data-attempt')).toBe('attempt-a');
  expect(document.activeElement).toBe(summary);
  expect(screen.getByRole('complementary').textContent).toContain('Exact frozen request ten');
  expect(row('SystemPromptCell', selected.id)).not.toBeNull();
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: 'request-10' } });
  expect(row('SystemPromptCell', selected.id).getAttribute('aria-selected')).toBe('true');
  fireEvent.change(search, { target: { value: 'request-1' } });
  fireEvent.change(search, { target: { value: '' } });
  expect(row('SystemPromptCell', selected.id)).not.toBeNull();
  fireEvent.click(turnSummary('attempt-a')!);
  expect(row('SystemPromptCell', selected.id).getAttribute('aria-selected')).toBe('true');
  fireEvent.click(screen.getByRole('button', { name: 'Close details' }));
  expect(document.activeElement).toBe(row('SystemPromptCell', selected.id));
  expect(load).not.toHaveBeenCalled();
});

it('407: System Prompt cells expose semantic tabs and preserve unknown historical evidence', () => {
  const initial = richRequest();
  const view = show(completeTraceDetail(cacheOf([initial]), initial.id, 1, requestDetail(0)));
  fireEvent.click(row('SystemPromptCell'));
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['System Prompt', 'Tools']);
  expect(screen.queryByRole('tab', { name: 'Diff' })).toBeNull();
  const unknown = richRequest();
  unknown.request!.system_prompt.state = 'previous_unavailable';
  unknown.request!.tool_catalog = 'previous_unavailable';
  const detail = requestDetail(0);
  detail.request!.previous_system_prompt = null;
  detail.request!.predecessor = { availability: 'unavailable', request_id: 'historical-predecessor' };
  view.rerender(<Trajectory cache={completeTraceDetail(cacheOf([unknown]), unknown.id, 1, detail)} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  fireEvent.click(row('SystemPromptCell'));
  expect(screen.queryByRole('tab', { name: 'Diff' })).toBeNull();
  expect(row('SystemPromptCell').textContent).toContain('Previous System Prompt unavailable');
  expect(screen.getByRole('tabpanel').textContent).toContain('You are the historical agent.');
  expect(screen.getByRole('tabpanel').textContent).not.toMatch(/unchanged|No changes/);
});

it.each([
  ['Step 2', 'GroupHeader', 'Step 2', 'attempt-a', ['trace:5', 'trace:8']],
  ['beta', 'GroupHeader', 'Step 2', 'attempt-a', ['trace:5', 'trace:8']],
  ['Turn 2', 'TurnHeader', 'Turn 2', 'attempt-b', ['trace:7']],
  ['attempt-b', 'TurnHeader', 'Turn 2', 'attempt-b', ['trace:7']],
  ['Message', 'GroupHeader', 'Message', 'attempt-a', ['trace:2']],
] as const)('structural search %s shares exact ledger/timeline membership without reads or collapse mutation', (query, type, label, attempt, ids) => {
  const records = structuralSearchRecords();
  const items = flattenTrajectory(translator('en'), projectTrajectory(translator('en'), records), 'older');
  const collapsed = new Set(['attempt-a', 'attempt-b']);
  const before = visibleItems(translator('en'), items, records, new Set(), null);
  const matches = searchItems(projectTrajectory(translator('en'), records), query);
  expect(matchedRecordIds(items, matches)).toEqual(new Set(ids));
  const exposed = visibleItems(translator('en'), items, records, new Set(), matches);
  expect(exposed.filter(isInspectable).map(item => item.owner_record_id)).toEqual([...ids]);
  expect(exposed).toContainEqual(expect.objectContaining({ type, label, attempt_id: attempt }));
  expect(visibleItems(translator('en'), items, records, new Set(), null)).toEqual(before);
  expect([...collapsed]).toEqual(['attempt-a', 'attempt-b']);

  const load = vi.fn(); const older = vi.fn();
  show(replaceTrace({ records, next_cursor: 'older' }), load, older);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  const previousKeys = () => [...ledger.querySelectorAll('[data-display-key]')].map(el => el.getAttribute('data-display-key'));
  const foldedKeys = previousKeys();
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: query } });
  // As in Harness, only a Turn has chrome; a Step or Message query exposes its
  // member cells without a header of its own.
  if (type === 'TurnHeader') expect(within(ledger).getByLabelText(label, { selector: 'span' }).closest('[data-attempt]')?.getAttribute('data-attempt')).toBe(attempt);
  else { expect(within(ledger).queryByLabelText(label, { selector: 'span' })).toBeNull(); expect(within(ledger).queryByRole('row', { name: label })).toBeNull(); }
  expect([...ledger.querySelectorAll('[data-owner]')].map(el => el.getAttribute('data-owner'))).toEqual([...ids]);
  const spans = [...document.querySelectorAll('[data-record-id]')];
  expect(spans.length).toBeGreaterThan(0);
  for (const span of spans) expect(span.getAttribute('data-search-match')).toBe(new Set<string>(ids).has(span.getAttribute('data-record-id')!) ? 'true' : 'false');
  fireEvent.change(search, { target: { value: '' } });
  expect(previousKeys()).toEqual(foldedKeys);
  expect(load).not.toHaveBeenCalled();
  expect(older).not.toHaveBeenCalled();
});

it('search conversion covers every inspectable cell, deduplicates owners and excludes history boundaries', () => {
  const assistant = traceRecord(1, { kind: 'assistant', request: null, message_id: 'assistant', calls: [{ call_id: 'call-2', tool_id: 'tool-bash', name: 'bash' }] });
  const records = [richRequest(), assistant, traceTool(2)];
  const items = flattenTrajectory(translator('en'), projectTrajectory(translator('en'), records), 'older');
  const folded = visibleItems(translator('en'), items, records, new Set([assistant.id]), null);
  const universe = [...items, ...folded];
  expect(new Set(universe.filter(isInspectable).map(item => item.type))).toEqual(new Set(['SystemPromptCell', 'ContextRow', 'RequestBoundary', 'RecordRow']));
  for (const item of universe.filter(isInspectable)) {
    expect(matchedRecordIds(universe, new Set([item.display_key]))).toEqual(new Set([item.owner_record_id]));
  }
  expect(matchedRecordIds(universe, new Set(universe.filter(isInspectable).map(item => item.display_key)))).toEqual(new Set(records.map(record => record.id)));
  expect(matchedRecordIds(items, new Set([items[0]!.display_key]))).toEqual(new Set());
  expect(matchedRecordIds(items, new Set())).toEqual(new Set());
  expect(matchedRecordIds(items, null)).toBeNull();
});

it.each([
  ['Frozen prompt', 'SystemPromptCell', 'trace:0'],
  ['Preview context-a', 'ContextRow', 'trace:0'],
  ['unique-model', 'RequestBoundary', 'trace:0'],
  ['ls -la', 'RecordRow', 'trace:1'],
] as const)('semantic search %s exposes only its exact cell and preserves Inspector ownership', (query, type, owner) => {
  const request = richRequest(); request.request!.model = 'unique-model';
  const records = [request, execution(1)];
  const items = trajectoryItems(records);
  const matches = searchItems(projectTrajectory(translator('en'), records), query)!;
  const expected = items.filter(item => isInspectable(item) && matches.has(item.display_key));
  expect(expected).toHaveLength(1);
  expect(expected[0]).toMatchObject({ type, owner_record_id: owner });
  const load = vi.fn(); const older = vi.fn();
  show(completeTraceDetail(cacheOf(records), request.id, 1, requestDetail(0)), load, older);
  fireEvent.click(row('SystemPromptCell'));
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: query } });
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect([...ledger.querySelectorAll('[data-owner]')].map(el => el.getAttribute('data-display-key'))).toEqual(ledgerRows(translator('en'), projectTrajectory(translator('en'), records), [...items.filter(item => !isInspectable(item)), ...expected], new Set(), true).filter(row => row.item).map(row => row.display_key));
  expect([...document.querySelectorAll('[data-record-id][data-search-match="true"]')].map(el => el.getAttribute('data-record-id'))).toEqual([type === 'SystemPromptCell' || type === 'ContextRow' ? expected[0].display_key : owner]);
  expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
  fireEvent.change(search, { target: { value: '' } });
  expect(row('SystemPromptCell').getAttribute('aria-selected')).toBe('true');
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
});

it('structural search overrides Calls without changing the stored collapse set', () => {
  const load = vi.fn(); const older = vi.fn();
  show(cacheOf([proposal(), execution(1)]), load, older);
  fireEvent.click(within(screen.getByRole('toolbar')).getByRole('button', { name: 'Collapse calls' }));
  expect(row('RecordRow', 'trace:1')).toBeNull();
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: 'Step 1' } });
  expect(row('RecordRow', 'trace:1')).not.toBeNull();
  expect(document.querySelector('[data-display-type="CallsSummary"]')).toBeNull();
  fireEvent.change(search, { target: { value: '' } });
  expect(row('RecordRow', 'trace:1')).toBeNull();
  expect(document.querySelector('[data-display-type="CallsSummary"]')).not.toBeNull();
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
});

it.each(['Turn 2', 'Step 2'])('%s reveals every semantic cell of its structural members', query => {
  const request = richRequest(5); request.location = { attempt_id: 'b', step_id: 'second' };
  const records = [traceRecord(0), traceRecord(1, { location: { attempt_id: 'b', step_id: 'first' } }), request];
  show(cacheOf(records));
  fireEvent.click(screen.getByRole('button', { name: 'Collapse turns' }));
  fireEvent.change(screen.getByRole('searchbox', { name: 'Search trajectory' }), { target: { value: query } });
  const expected = trajectoryItems(query === 'Turn 2' ? records.slice(1) : [request]);
  const items = trajectoryItems(records);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect([...ledger.querySelectorAll('[data-owner]')].map(el => el.getAttribute('data-display-key'))).toEqual(ledgerRows(translator('en'), projectTrajectory(translator('en'), records), [...items.filter(item => !isInspectable(item)), ...expected], new Set(), true).filter(row => row.item).map(row => row.display_key));
  expect([...document.querySelectorAll('[data-record-id][data-search-match="true"]')].map(el => el.getAttribute('data-record-id')).sort()).toEqual([...expected.filter(isInspectable).filter(item => item.type === 'SystemPromptCell' || item.type === 'ContextRow').map(item => item.display_key), ...(query === 'Turn 2' ? ['trace:1', 'trace:5'] : ['trace:5'])].sort());
});

const epochRecords = (...ns: number[]) => ns.map(n => traceRecord(n, { location: { attempt_id: `attempt-${n}`, step_id: 'step' } }));

it('407: Timeline focus keeps native identity across prepend and lifecycle refresh within one Trace epoch', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const view = show(initial);
  expect(timelineFocusOf()).toEqual({ 'trace:0': undefined, 'trace:1': undefined, 'trace:2': undefined, 'trace:3': undefined });
  dragTimeline(30, 70);
  expect(timelineFocusOf()).toEqual({ 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'inside', 'trace:3': 'outside' });
  const before = overlayLeft();
  const canvas = timelineCanvas();
  const prepended = prependTrace(initial, { records: epochRecords(9), next_cursor: null });
  expect(prepended.epoch).toBe(initial.epoch);
  view.rerender(<Trajectory cache={prepended} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  // Same native identities; the Turn ordinal and projected range both moved.
  expect(timelineFocusOf()).toEqual({ 'trace:9': 'outside', 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'inside', 'trace:3': 'outside' });
  expect(screen.getByLabelText('Turn 3', { selector: 'span' }).closest('[data-attempt]')?.getAttribute('data-attempt')).toBe('attempt-1');
  expect(overlayLeft()).not.toBe(before);
  // Projection changes retire coordinates while committed native focus survives.
  expect(timelineCanvas()).not.toBe(canvas);
  const prependedCanvas = timelineCanvas();
  const refreshed = refreshTrace(prepended, { records: [{ ...prepended.page.records[4]!, state: 'failed' }], next_cursor: null });
  expect(refreshed.epoch).toBe(initial.epoch);
  view.rerender(<Trajectory cache={refreshed} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(timelineFocusOf()).toEqual({ 'trace:9': 'outside', 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'inside', 'trace:3': 'outside' });
  expect(focusOverlay()).not.toBeNull();
  expect(timelineCanvas()).toBe(prependedCanvas);
});

it('407: a Trace epoch rebase retires Timeline focus even when record identities recur', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const view = show(initial);
  dragTimeline(30, 70);
  expect(focusOverlay()).not.toBeNull();
  const replaced = replaceTrace({ records: epochRecords(20, 21, 22, 23), next_cursor: null }, initial);
  expect(replaced.epoch).toBe(initial.epoch + 1);
  view.rerender(<Trajectory cache={replaced} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(focusOverlay()).toBeNull();
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  // The epoch, not identity absence, owns the lifetime: recurring IDs stay unfocused.
  const recurring = replaceTrace({ records: epochRecords(0, 1, 2, 3), next_cursor: null }, replaced);
  view.rerender(<Trajectory cache={recurring} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(focusOverlay()).toBeNull();
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  // A focus created in the new domain belongs to it.
  dragTimeline(55, 95);
  expect(timelineFocusOf()).toEqual({ 'trace:0': 'outside', 'trace:1': 'outside', 'trace:2': 'inside', 'trace:3': 'inside' });
});

const domainOf = () => { const canvas = timelineCanvas(); return [canvas.dataset.domainStart, canvas.dataset.domainEnd]; };

it.each([
  ['unrelated', epochRecords(20, 21, 22, 23)],
  ['recurring', epochRecords(0, 1, 2, 3)],
] as const)('407: an in-flight E1 Timeline drag cannot commit into E2 with %s record IDs and the same numeric domain', (_, next) => {
  const select = vi.fn();
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const render = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} onSelect={select} onLoadDetail={noop} />;
  const view = show(initial);
  view.rerender(render(initial));
  const domain = domainOf();
  const stale = timelineCanvas();
  fireEvent.pointerDown(stale, { button: 0, pointerId: 1, clientX: 30 });
  fireEvent.pointerMove(stale, { pointerId: 1, clientX: 45 });
  // The uncommitted draft is visible, but no focus exists yet.
  expect(focusOverlay()).not.toBeNull();
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  const replaced = replaceTrace({ records: [...next], next_cursor: null }, initial);
  expect(replaced.epoch).toBe(initial.epoch + 1);
  view.rerender(render(replaced));
  // Identical projected coordinates: a coordinate-derived reset key cannot tell E1 from E2.
  expect(domainOf()).toEqual(domain);
  expect(focusOverlay()).toBeNull();
  // The old gesture's release reaches both the detached E1 canvas and the E2 canvas under the pointer.
  fireEvent.pointerUp(stale, { pointerId: 1, clientX: 45 });
  fireEvent.pointerUp(timelineCanvas(), { pointerId: 1, clientX: 45 });
  expect(focusOverlay()).toBeNull();
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  expect(document.querySelectorAll('[aria-selected="true"]')).toHaveLength(0);
  expect(screen.queryByRole('complementary')).toBeNull();
  expect(select).not.toHaveBeenCalled();
  // Only a gesture begun in E2 can focus E2.
  dragTimeline(30, 70);
  const ids = next.map(record => record.id);
  expect(timelineFocusOf()).toEqual({ [ids[0]!]: 'outside', [ids[1]!]: 'inside', [ids[2]!]: 'inside', [ids[3]!]: 'outside' });
});

it('407: a pressed E1 span cannot select a recurring E2 record after a rebase', () => {
  const select = vi.fn(); const load = vi.fn();
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const render = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} onSelect={select} onLoadDetail={load} />;
  const view = show(initial);
  view.rerender(render(initial));
  timelineCanvas();
  fireEvent.pointerDown(document.querySelector('[data-record-id="trace:1"]')!, { button: 0, pointerId: 1, clientX: 30 });
  view.rerender(render(replaceTrace({ records: epochRecords(0, 1, 2, 3), next_cursor: null }, initial)));
  fireEvent.pointerUp(timelineCanvas(), { pointerId: 1, clientX: 30 });
  expect(select).not.toHaveBeenCalled(); expect(load).not.toHaveBeenCalled();
  expect(document.querySelectorAll('[aria-selected="true"]')).toHaveLength(0);
});

it('407: Timeline zoom, pan and hover belong to the Trace epoch even across an identical numeric domain', () => {
  // Eight sequence spans: large enough to zoom above the minimum viewport span.
  const initial = cacheOf(epochRecords(0, 1, 2, 3, 4, 5, 6, 7));
  const view = show(initial);
  const hoverLine = () => document.querySelector('[data-timeline-hover-line]');
  const full = domainOf();
  zoomTimeline();
  const canvas = timelineCanvas();
  fireEvent.pointerDown(canvas, { button: 2, pointerId: 3, clientX: 50 });
  fireEvent.pointerMove(canvas, { pointerId: 3, clientX: 30 });
  fireEvent.pointerUp(canvas, { pointerId: 3, clientX: 30 });
  const zoomed = domainOf();
  expect(zoomed).not.toEqual(full);
  fireEvent.pointerMove(canvas, { pointerId: 4, clientX: 60 });
  expect(hoverLine()).not.toBeNull();
  // E2 reuses the IDs and the numeric domain; only the epoch differs.
  const replaced = replaceTrace({ records: epochRecords(0, 1, 2, 3, 4, 5, 6, 7), next_cursor: null }, initial);
  view.rerender(<Trajectory cache={replaced} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(domainOf()).toEqual(full);
  expect(hoverLine()).toBeNull();
  expect(focusOverlay()).toBeNull();
});

/** Native timing refresh grows an inner span without moving the outer domain. */
function timingRevision() {
  const records = [0, 1, 2].map(n => traceTool(n, {
    timing: { started_at: `2026-09-15T00:00:0${n * 2}Z`, ended_at: `2026-09-15T00:00:0${n * 2 + 1}Z`, duration_ms: '1000' },
  }));
  const initial = cacheOf(records);
  const next = refreshTrace(initial, { records: [{ ...records[1]!, state: 'failed',
    timing: { ...records[1]!.timing, ended_at: '2026-09-15T00:00:04Z', duration_ms: '2000' } }], next_cursor: null });
  return { initial, next };
}

it.each(['sequence', 'duration'] as const)('407: in-flight %s drag cannot cross a same-epoch projection revision', mode => {
  const initialSequence = cacheOf(epochRecords(0, 1, 2, 3));
  const { initial, next } = mode === 'sequence'
    ? { initial: initialSequence, next: prependTrace(initialSequence, { records: epochRecords(9), next_cursor: null }) }
    : timingRevision();
  const p1 = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), initial.page.records), mode)!;
  const p2 = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), next.page.records), mode)!;
  expect(next.epoch).toBe(initial.epoch);
  expect(timelineProjectionRevision(p2, mode)).not.toBe(timelineProjectionRevision(p1, mode));
  const oldRange = mode === 'sequence' ? { start: 1.2, end: 1.8 }
    : { start: p1.start + 3100, end: p1.start + 3200 };
  expect(timelineFocus(p1, oldRange)).not.toEqual(timelineFocus(p2, oldRange));
  const select = vi.fn(); const load = vi.fn();
  const ui = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} onSelect={select} onLoadDetail={load} />;
  const view = render(ui(initial));
  if (mode !== 'sequence') fireEvent.click(screen.getByRole('button', { name: 'Use actual duration' }));
  const stale = timelineCanvas();
  fireEvent.pointerDown(stale, { button: 0, pointerId: 1, clientX: 30 });
  fireEvent.pointerMove(stale, { pointerId: 1, clientX: 45 });
  expect(focusOverlay()).not.toBeNull();
  view.rerender(ui(next));
  expect(timelineCanvas()).not.toBe(stale);
  expect(focusOverlay()).toBeNull();
  fireEvent.pointerUp(stale, { pointerId: 1, clientX: 45 });
  fireEvent.pointerUp(timelineCanvas(), { pointerId: 1, clientX: 45 });
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  expect(document.querySelectorAll('[aria-selected="true"]')).toHaveLength(0);
  expect(screen.queryByRole('complementary')).toBeNull();
  expect(select).not.toHaveBeenCalled(); expect(load).not.toHaveBeenCalled();
  // As in Harness, a drag narrower than one block widens to one centred
  // block, so its inclusive edges also reach the touching neighbours.
  dragTimeline(mode === 'sequence' ? 45 : 30, mode === 'sequence' ? 55 : 60);
  expect(Object.entries(timelineFocusOf()).filter(([, focus]) => focus === 'inside').map(([id]) => id)).toEqual(mode === 'sequence' ? ['trace:0', 'trace:1', 'trace:2'] : ['trace:1']);
  expect(select).not.toHaveBeenCalled();
});

it('407: obsolete same-epoch span press and synthesized pointer click cannot open Inspector', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const select = vi.fn(); const load = vi.fn();
  const ui = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} onSelect={select} onLoadDetail={load} />;
  const view = render(ui(initial));
  const staleCanvas = timelineCanvas();
  const staleSpan = document.querySelector('[data-record-id="trace:1"]')!;
  fireEvent.pointerDown(staleSpan, { button: 0, pointerId: 1, clientX: 30 });
  const next = prependTrace(initial, { records: epochRecords(9), next_cursor: null });
  view.rerender(ui(next));
  fireEvent.pointerUp(staleCanvas, { pointerId: 1, clientX: 30 });
  fireEvent.pointerUp(timelineCanvas(), { pointerId: 1, clientX: 30 });
  fireEvent.click(staleSpan, { detail: 1 });
  const freshSpan = document.querySelector('[data-record-id="trace:1"]')!;
  fireEvent.click(freshSpan, { detail: 1 });
  expect(select).not.toHaveBeenCalled(); expect(load).not.toHaveBeenCalled();
  expect(screen.queryByRole('complementary')).toBeNull();
  fireEvent.pointerDown(freshSpan, { button: 0, pointerId: 2, clientX: 50 });
  fireEvent.pointerUp(timelineCanvas(), { pointerId: 2, clientX: 50 });
  expect(select).toHaveBeenCalledWith('trace:1');
  expect(screen.getByRole('complementary')).toBeTruthy();
});

it('407: an obsolete pan cannot mutate a newly zoomed same-epoch viewport', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3, 4, 5, 6, 7));
  const ui = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />;
  const view = render(ui(initial));
  zoomTimeline();
  const stale = timelineCanvas();
  fireEvent.pointerDown(stale, { button: 2, pointerId: 1, clientX: 50 });
  fireEvent.pointerMove(stale, { pointerId: 1, clientX: 40 });
  const next = prependTrace(initial, { records: epochRecords(9), next_cursor: null });
  view.rerender(ui(next));
  expect(domainOf()).toEqual(['0', '9']);
  zoomTimeline();
  const fresh = timelineCanvas();
  const domain = domainOf();
  fireEvent.pointerMove(stale, { pointerId: 1, clientX: 20 });
  fireEvent.pointerMove(fresh, { pointerId: 1, clientX: 20 });
  fireEvent.pointerUp(fresh, { pointerId: 1, clientX: 20 });
  expect(domainOf()).toEqual(domain);
  fireEvent.pointerDown(fresh, { button: 2, pointerId: 2, clientX: 50 });
  fireEvent.pointerMove(fresh, { pointerId: 2, clientX: 40 });
  fireEvent.pointerUp(fresh, { pointerId: 2, clientX: 40 });
  expect(domainOf()).not.toEqual(domain);
});

it('407: equivalent coordinate projections preserve a gesture through status refresh', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const view = show(initial);
  const canvas = timelineCanvas();
  fireEvent.pointerDown(canvas, { button: 0, pointerId: 1, clientX: 30 });
  fireEvent.pointerMove(canvas, { pointerId: 1, clientX: 70 });
  const next = refreshTrace(initial, { records: [{ ...initial.page.records[1]!, state: 'failed' }], next_cursor: null });
  expect(timelineProjectionRevision(trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), initial.page.records), 'sequence'), 'sequence'))
    .toBe(timelineProjectionRevision(trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), next.page.records), 'sequence'), 'sequence'));
  view.rerender(<Trajectory cache={next} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(timelineCanvas()).toBe(canvas);
  fireEvent.pointerUp(canvas, { pointerId: 1, clientX: 70 });
  expect(timelineFocusOf()).toEqual({ 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'inside', 'trace:3': 'outside' });
});

it('407: committed native focus survives a timing revision', () => {
  const { initial, next } = timingRevision();
  const view = show(initial);
  fireEvent.click(screen.getByRole('button', { name: 'Use actual duration' }));
  dragTimeline(5, 30);
  const before = timelineFocusOf();
  expect(before).toEqual({ 'trace:0': 'inside', 'trace:1': 'inside', 'trace:2': 'outside' });
  const width = overlayWidth();
  view.rerender(<Trajectory cache={next} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(timelineFocusOf()).toEqual(before);
  expect(overlayWidth()).not.toBe(width);
});


it('locale changes preserve Trajectory search membership for both vocabularies and native facts', () => {
  const records = [...structuralSearchRecords(), richRequest()];
  const english = projectTrajectory(translator('en'), records);
  const chinese = projectTrajectory(translator('zh'), records);
  for (const query of ['Step 2', 'Turn 2', 'Message', 'beta', 'Frozen prompt', 'Preview context-a',
    translator('zh')('trajectory:group.step', { n: 2 }),
    translator('zh')('trajectory:copy.turn-value', { p0: 2 }),
    translator('zh')('trajectory:layout.initial-system-prompt')]) {
    const expected = searchItems(english, query);
    expect(expected?.size, query).toBeGreaterThan(0);
    expect(searchItems(chinese, query), query).toEqual(expected);
  }
});

it('ContextRow labels localize the native context kind while the kind and search membership stay locale-independent', () => {
  const record = richRequest();
  record.request!.context_additions = [context('context-a'), { ...context('context-goal'), context_kind: 'goal_status' }];
  const en = translator('en'); const zh = translator('zh');
  const english = projectTrajectory(en, [record]); const chinese = projectTrajectory(zh, [record]);
  const rows = (tx: typeof en, projection: typeof english) => flattenTrajectory(tx, projection)
    .flatMap(item => item.type === 'ContextRow' ? [{ key: item.display_key, label: item.label, kind: item.context.context_kind }] : []);
  const [native, goal] = rows(en, english);
  expect(rows(en, english)).toEqual([
    { key: native!.key, label: 'Native context', kind: 'native_environment' },
    { key: goal!.key, label: 'Goal status', kind: 'goal_status' },
  ]);
  expect(rows(zh, chinese)).toEqual([
    { key: native!.key, label: zh('trajectory:context.native_environment'), kind: 'native_environment' },
    { key: goal!.key, label: zh('trajectory:context.goal_status'), kind: 'goal_status' },
  ]);
  expect(zh('trajectory:context.goal_status')).not.toBe('Goal status');
  for (const query of [en('trajectory:context.goal_status'), zh('trajectory:context.goal_status'), 'goal_status']) {
    expect(searchItems(english, query), query).toEqual(new Set([goal!.key]));
    expect(searchItems(chinese, query), query).toEqual(new Set([goal!.key]));
  }

  render(<Trajectory cache={cacheOf([record])} loadEarlier={noop} onSelect={noop} onLoadDetail={noop} />);
  // As in Harness, a Context row shows its content beside a localized role tag.
  const goalRow = () => [...document.querySelectorAll<HTMLElement>('[data-display-type="ContextRow"]')].find(node => node.dataset.displayKey === goal!.key)!;
  expect(goalRow().textContent).toBe('CONTEXTPreview context-goal');
  act(() => localeController.setLocale('zh'));
  expect(goalRow().textContent).toBe(`${zh('trajectory:kind.context')}Preview context-goal`);
  act(() => localeController.setLocale('en'));
  expect(record.request!.context_additions.map(addition => addition.context_kind)).toEqual(['native_environment', 'goal_status']);
});

it('Tool facet read errors keep the facet identity across locales and render the native error byte-for-byte', async () => {
  const { reads, pending } = controlledDetails([traceTool(10)]);
  fireEvent.click(row('RecordRow', 'trace:10'));
  fireEvent.click(screen.getByRole('tab', { name: 'Result' }));
  const error = 'Controlled read failure: Input Result Schema 原始 <raw>';
  await act(async () => pending.get('trace:10')!.reject(new Error(error)));
  const zh = translator('zh');
  act(() => localeController.setLocale('zh'));
  expect(screen.getByRole('tab', { name: zh('trajectory:tab.result') }).getAttribute('aria-selected')).toBe('true');
  for (const facet of ['payload', 'result', 'schema'] as const) {
    fireEvent.click(screen.getByRole('tab', { name: zh(`trajectory:tab.${facet}`) }));
    expect(within(screen.getByRole('tabpanel')).getByRole('alert').textContent).toBe(error);
  }
  act(() => localeController.setLocale('en'));
  expect(screen.getByRole('tab', { name: 'Schema' }).getAttribute('aria-selected')).toBe('true');
  expect(within(screen.getByRole('tabpanel')).getByRole('alert').textContent).toBe(error);
  expect(reads).toEqual(['trace:10']);
});

it('folded Turn summary and Status localize without changing membership, native state or reads', () => {
  const records = [
    traceRecord(0, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } }),
    traceRecord(1, { kind: 'assistant', request: null, location: { attempt_id: 'attempt-a', step_id: '1' } }),
    traceTool(2, { location: { attempt_id: 'attempt-a', step_id: '1' }, state: 'failed' }),
    traceTool(9, { location: { attempt_id: 'attempt-b', step_id: '1' }, state: 'denied' }),
  ];
  const load = vi.fn(); const older = vi.fn(); const select = vi.fn();
  render(<Trajectory cache={cacheOf(records)} loadEarlier={older} onSelect={select} onLoadDetail={load} />);
  foldTurn('Turn 1');
  fireEvent.click(row('RecordRow', 'trace:9'));
  const folded = turnSummary('attempt-a')!;
  const preview = () => folded.querySelectorAll('[role="cell"]')[1]!.textContent;
  const owners = () => [...document.querySelectorAll<HTMLElement>('[data-owner]')].map(element => element.dataset.owner);
  const status = () => within(screen.getByRole('complementary')).getByText(translator(localeController.getSnapshot().active)('trajectory:details.status'), { selector: 'dt' }).nextElementSibling?.textContent;
  expect(preview()).toBe('…1 step · 1 tool call');
  expect(status()).toBe('Failed');
  const visible = owners();
  expect(visible).toEqual(['trace:0', 'trace:9']);
  const calls = [load, older, select].map(fn => fn.mock.calls.length);

  act(() => localeController.setLocale('zh'));
  expect(preview()).toBe('…1 个步骤 · 1 个工具调用');
  expect(status()).toBe('失败');
  expect(owners()).toEqual(visible);
  expect(records.map(record => record.state)).toEqual(['completed', 'completed', 'failed', 'denied']);
  expect([load, older, select].map(fn => fn.mock.calls.length)).toEqual(calls);

  act(() => localeController.setLocale('en'));
  expect(preview()).toBe('…1 step · 1 tool call');
  expect(owners()).toEqual(visible);
  expect([load, older, select].map(fn => fn.mock.calls.length)).toEqual(calls);
});

it('Harness ledger: each Turn opens on its own input, with the Request marker above its first output', () => {
  const at = (attempt_id: string, step_id?: string) => ({ attempt_id, ...(step_id ? { step_id } : {}) });
  const first = richRequest(2); first.location = at('attempt-a', '1');
  const second = traceRecord(6, { location: at('attempt-b', '1') });
  const records = [
    traceRecord(1, { kind: 'user', request: null, location: at('attempt-a') }), first,
    traceRecord(3, { kind: 'assistant', request: null, location: at('attempt-a', '1') }),
    traceRecord(5, { kind: 'user', request: null, location: at('attempt-b') }), second,
    traceRecord(7, { kind: 'assistant', request: null, location: at('attempt-b', '1') }),
  ];
  const rows = ledgerRows(translator('en'), projectTrajectory(translator('en'), records), trajectoryItems(records), new Set(), false);
  expect(rows.map(row => [row.item?.type, row.item?.owner_record_id, row.turnStart, row.request?.owner_record_id ?? null])).toEqual([
    ['SystemPromptCell', 'trace:2', false, null],
    ['RecordRow', 'trace:1', true, null],
    ['ContextRow', 'trace:2', false, null], ['ContextRow', 'trace:2', false, null],
    ['RecordRow', 'trace:3', false, 'trace:2'],
    ['RecordRow', 'trace:5', true, null],
    ['RecordRow', 'trace:7', false, 'trace:6'],
  ]);
  expect(rows.every(row => row.kind === 'semantic')).toBe(true);
  show(cacheOf(records));
  // As in Harness, Steps carry no chrome of their own.
  expect(document.querySelector('[data-structural="step"]')).toBeNull();
  fireEvent.click(row('RecordRow', 'trace:3'));
  // The promoted initial prompt sits outside the active Turn's rail.
  expect(row('SystemPromptCell', 'trace:2').querySelector('[class*="turnRail"]')).toBeNull();
  expect(row('RecordRow', 'trace:1').querySelector('[class*="turnRail"]')).not.toBeNull();
  expect(row('SystemPromptCell', 'trace:2').textContent).toContain('Initial System Prompt');
  expect(row('SystemPromptCell', 'trace:2').textContent).not.toContain('Frozen prompt');
});

it('421: ledger seats carry exact native actions and System precedes Turn chrome without heavyweight structure', () => {
  const request = richRequest(2);
  const retry = traceRecord(3);
  const user = traceRecord(1, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } });
  const assistant = traceRecord(4, { kind: 'assistant', request: null });
  const contextRequest = richRequest(5);
  contextRequest.request!.system_prompt.state = 'unchanged';
  contextRequest.request!.tool_catalog = 'unchanged';
  const records = [user, request, retry, contextRequest, assistant];
  const rows = ledgerRows(translator('en'), projectTrajectory(translator('en'), records), trajectoryItems(records), new Set(), false);
  expect(rows[0]!.item?.type).toBe('SystemPromptCell');
  expect(rows[0]!.item?.record.location).toEqual(request.location);
  expect(rows[0]!.turnStart).toBe(false);
  expect(rows[1]!.turnStart).toBe(true);
  // As in Harness, a Request's marker sits above the first output of its own
  // Step; a Request followed by another Request keeps its own marker seat.
  expect(rows[0]!.request).toBeUndefined();
  for (const owner of [request, retry]) {
    const seat = rows.find(row => row.request?.owner_record_id === owner.id)!;
    expect(seat.kind).toBe('marker'); expect(seat.height).toBe(10);
    expect(seat.item?.record.request?.request_id).toBe(owner.request!.request_id);
  }
  // As in Harness, consecutive Request dots step right instead of stacking.
  expect([request, retry].map(owner => rows.find(row => row.request?.owner_record_id === owner.id)!.requestRun)).toEqual([0, 1]);
  const outputSeat = rows.find(row => row.request?.owner_record_id === contextRequest.id)!;
  expect(outputSeat.requestRun).toBe(0);
  expect(outputSeat.item?.owner_record_id).toBe(assistant.id);
  expect(outputSeat.request?.record.request?.request_id).toBe(contextRequest.request!.request_id);
  expect(rows.filter(row => row.request?.owner_record_id === contextRequest.id)).toHaveLength(1);
  expect(rows.find(row => row.item?.type === 'ContextRow' && row.item.owner_record_id === contextRequest.id)?.request).toBeUndefined();
  const load = vi.fn(); show(cacheOf(records), load);
  expect(document.querySelectorAll('[role="row"][data-display-type="TurnHeader"], [role="row"][data-display-type="GroupHeader"], [role="row"][data-display-type="RequestBoundary"]')).toHaveLength(0);
  expect(screen.queryByRole('button', { name: 'Turn 1' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Step 1' })).toBeNull();
  expect(load).not.toHaveBeenCalled();
  fireEvent.click(row('RequestBoundary', retry.id));
  expect(load.mock.calls).toEqual([[retry.id]]);
});

it('421: fold retains System, first main semantic content and actionable compact summary; bilingual search restores exact folds', () => {
  const user = traceRecord(1, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } });
  const records = [user, richRequest(2), traceTool(3), traceRecord(4, { kind: 'assistant', request: null })];
  const load = vi.fn(); show(cacheOf(records), load);
  foldTurn('Turn 1');
  expect(row('SystemPromptCell', 'trace:2')).not.toBeNull();
  expect(row('RecordRow', user.id)).not.toBeNull();
  expect(row('RecordRow', 'trace:3')).toBeNull();
  expect(document.querySelector('[data-display-type="TurnSummary"]')?.getBoundingClientRect).toBeDefined();
  const keys = () => [...screen.getByRole('table').querySelectorAll('[data-display-key]')].map(el => el.getAttribute('data-display-key'));
  const before = keys();
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: 'Step 1' } });
  expect(row('RecordRow', 'trace:3')).not.toBeNull();
  const english = keys();
  act(() => localeController.setLocale('zh'));
  expect(keys()).toEqual(english);
  fireEvent.change(search, { target: { value: '' } });
  expect(keys()).toEqual(before);
  expect(load).not.toHaveBeenCalled();
  act(() => localeController.setLocale('en'));
});

it('421: Tool input and result render bounded summary facts without detail reads', () => {
  const tool = traceTool(1, { preview: { text: 'bounded result', truncated: true } });
  tool.tool!.arguments = { text: '{"command":"bounded input', truncated: true };
  const load = vi.fn(); show(cacheOf([tool]), load);
  expect(row('RecordRow', tool.id).textContent).toContain('bounded input');
  expect(row('RecordRow', tool.id).textContent).toContain('→bounded result');
  fireEvent.change(screen.getByRole('searchbox', { name: 'Search trajectory' }), { target: { value: 'bounded input' } });
  expect(row('RecordRow', tool.id)).not.toBeNull();
  expect(load).not.toHaveBeenCalled();
});


it('424: logical arrows visit exact Request and semantic targets without structural detail reads', () => {
  const records = [
    traceRecord(0, { kind: 'user', request: null, location: {} }),
    traceRecord(1, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' } }),
    traceRecord(2, { kind: 'step', request: null }), traceRecord(3), traceRecord(4),
  ];
  const rows = ledgerRows(translator('en'), projectTrajectory(translator('en'), records), trajectoryItems(records), new Set(), false);
  // As in Harness, neither a Turn nor a Step is a navigation target.
  expect(ledgerFocusTargets(rows).map(target => target.item.type)).toEqual(['RecordRow', 'RequestBoundary', 'RequestBoundary']);
  const load = vi.fn(); show(cacheOf(records), load);
  expect(row('RequestBoundary', 'trace:3').closest('[role=row]')!.textContent).not.toContain('historical-model');
  expect(screen.queryByRole('button', { name: 'Step 1' })).toBeNull();
  const user = row('RecordRow', 'trace:0');
  act(() => user.focus());
  fireEvent.keyDown(user, { key: 'ArrowDown' }); expect(document.activeElement).toBe(row('RequestBoundary', 'trace:3'));
  expect(load.mock.calls).toEqual([['trace:3']]);
  fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' }); expect(document.activeElement).toBe(row('RequestBoundary', 'trace:4'));
  expect(load.mock.calls).toEqual([['trace:3'], ['trace:4']]);
  fireEvent.keyDown(row('RequestBoundary', 'trace:4'), { key: 'ArrowUp' });
  fireEvent.keyDown(row('RequestBoundary', 'trace:3'), { key: 'ArrowUp' });
  expect(document.activeElement).toBe(user);
});

it.each(['only-system', 'native-step', 'context', 'search'] as const)('424: promoted System precedes the Turn chrome with truthful fallback geometry: %s', scenario => {
  const request = richRequest(3);
  if (scenario !== 'context') request.request!.context_additions = [];
  const records = [traceRecord(1, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' } }),
    ...(scenario === 'only-system' ? [] : [traceRecord(2, { kind: 'step', request: null })]), request];
  if (scenario === 'context' || scenario === 'search') records.splice(1, 0, traceRecord(0, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } }));
  const items = trajectoryItems(records);
  const matches = scenario === 'search' ? searchItems(projectTrajectory(translator('en'), records), 'Frozen prompt') : null;
  const rows = ledgerRows(translator('en'), projectTrajectory(translator('en'), records), visibleItems(translator('en'), items, records, new Set(), matches), new Set(), !!matches);
  const system = rows.find(row => row.item?.type === 'SystemPromptCell')!;
  expect(system.item?.record.location).toEqual(request.location);
  // With no output yet, the Request keeps its own marker seat in its Step.
  expect(system.request).toBeUndefined();
  if (scenario !== 'search') expect(rows.find(row => row.request?.owner_record_id === request.id)!.item?.type).toBe('RequestBoundary');
  const turnIndex = rows.findIndex(row => row.turnStart);
  expect(turnIndex).toBeGreaterThan(rows.indexOf(system));
  const opening = rows[turnIndex]!;
  // A Request or structure seat that opens a Turn grows to hold its chrome.
  if (scenario === 'context') { expect(opening.item?.owner_record_id).toBe('trace:0'); expect(opening.height).toBe(30); }
  else { expect(opening.kind).toBe('structure'); expect(opening.height).toBe(20); expect(opening.item?.type).toBe(scenario === 'search' ? undefined : 'RequestBoundary'); }
  const load = vi.fn(); show(cacheOf(records), load);
  if (scenario === 'search') fireEvent.change(screen.getByRole('searchbox', { name: 'Search trajectory' }), { target: { value: 'Frozen prompt' } });
  expect(document.querySelector('[data-structural]')).toBeNull();
  expect(screen.getByLabelText('Turn 1', { selector: 'span' })).toBeDefined();
  expect(load).not.toHaveBeenCalled();
});

it('424: structural-only seats and request-only seats have distinct exact navigation and height contracts', () => {
  const records = [traceRecord(1, { kind: 'attempt', request: null, location: { attempt_id: 'empty' } }),
    traceRecord(2, { kind: 'step', request: null, location: { attempt_id: 'empty', step_id: 'empty-step' } }), traceRecord(3), traceRecord(4)];
  const rows = ledgerRows(translator('en'), projectTrajectory(translator('en'), records), trajectoryItems(records), new Set(), false);
  const empty = rows.find(row => row.turn?.attempt_id === 'empty')!;
  expect(empty.kind).toBe('structure'); expect(empty.height).toBe(20);
  expect(ledgerFocusTargets([empty])).toEqual([]);
  const requestOnly = rows.find(row => row.request?.owner_record_id === 'trace:4')!;
  expect(requestOnly.kind).toBe('marker'); expect(requestOnly.height).toBe(10);
  expect(ledgerFocusTargets([requestOnly]).map(target => target.item.type)).toEqual(['RequestBoundary']);
  const before = traceRecord(0, { location: { attempt_id: 'before', step_id: 'before-step' } });
  for (const folded of [new Set<string>(), new Set(['empty'])]) {
    const seats = ledgerRows(translator('en'), projectTrajectory(translator('en'), [before, ...records]), trajectoryItems([before, ...records]), folded, false);
    expect(seats.filter(row => row.turnStart).map(row => row.turn!.attempt_id)).toEqual(['before', 'empty', 'attempt-a']);
  }
});

it('424: closing the Inspector after folding hides its record leaves the fold actionable', () => {
  const user = traceRecord(1, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } });
  const load = vi.fn(); show(cacheOf([user, richRequest(2), traceTool(3)]), load);
  fireEvent.click(row('RecordRow', 'trace:3'));
  expect(load.mock.calls).toEqual([['trace:3']]);
  foldTurn('Turn 1');
  expect(row('RecordRow', 'trace:3')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Close details' }));
  expect(screen.queryByRole('complementary')).toBeNull();
  // As in Harness, Steps have no chrome, folded or expanded.
  expect(screen.queryByRole('button', { name: 'Step 1' })).toBeNull();
  fireEvent.keyDown(turnSummary('attempt-a')!, { key: 'Enter' });
  expect(row('RecordRow', 'trace:3')).not.toBeNull();
  expect(load.mock.calls).toEqual([['trace:3']]);
});


it.each([
  ['A initial-only first Step', true, 0],
  ['B empty middle Step', false, 1],
  ['C consecutive empty Steps', false, 2],
] as const)('424 order: %s keeps native group order and gives empty Steps no seat', (_name, initial, emptySteps) => {
  const records = orderedStepRecords(initial, emptySteps);
  const projection = projectTrajectory(translator('en'), records);
  const rows = ledgerRows(translator('en'), projection, trajectoryItems(records), new Set(), false);
  // As in Harness, a Step has no chrome: empty Steps add no rows or targets.
  expect(rows.map(row => [row.item?.type, row.item?.owner_record_id, row.turnStart])).toEqual(initial
    ? [['SystemPromptCell', 'trace:702', false], ['RequestBoundary', 'trace:702', true], ['RecordRow', 'trace:706', false]]
    : [['RecordRow', 'trace:702', true], ['RecordRow', 'trace:706', false]]);
  expect(ledgerFocusTargets(rows).map(target => target.item.type)).toEqual(initial ? ['SystemPromptCell', 'RequestBoundary', 'RecordRow'] : ['RecordRow', 'RecordRow']);
  const load = vi.fn(); show(cacheOf(records), load);
  expect(document.querySelector('[data-step]')).toBeNull();
  const first = initial ? row('SystemPromptCell', 'trace:702') : row('RecordRow', 'trace:702');
  act(() => first.focus());
  expect(load).not.toHaveBeenCalled();
  if (initial) { fireEvent.keyDown(first, { key: 'ArrowDown' }); expect(document.activeElement).toBe(row('RequestBoundary', 'trace:702')); }
  fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
  expect(document.activeElement).toBe(row('RecordRow', 'trace:706'));
});

it('424 order: search keeps native position and restores exact folds across locales', () => {
  const records = orderedStepRecords(true, 0, true);
  const projection = projectTrajectory(translator('en'), records);
  const items = trajectoryItems(records);
  const matches = searchItems(projection, 'order-match')!;
  const rows = ledgerRows(translator('en'), projection, visibleItems(translator('en'), items, records, new Set(), matches), new Set(['ordered-turn']), true);
  expect(rows.map(row => [row.item?.owner_record_id, row.turnStart])).toEqual([['trace:702', false], ['trace:706', true]]);
  expect(rows.some(row => row.item?.type === 'ContextRow')).toBe(false);
  const load = vi.fn(); show(cacheOf(records), load);
  foldTurn('Turn 1');
  const keys = () => [...screen.getByRole('table').querySelectorAll('[data-display-key]')].map(el => el.getAttribute('data-display-key'));
  const owners = () => [...screen.getByRole('table').querySelectorAll<HTMLElement>('[role="row"][data-owner]')].map(el => el.dataset.owner);
  const foldedKeys = keys();
  expect(turnSummary('ordered-turn')).not.toBeNull();
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: 'order-match' } });
  const searchingKeys = keys();
  expect(owners()).toEqual(['trace:702', 'trace:706']);
  act(() => localeController.setLocale('zh')); expect(keys()).toEqual(searchingKeys);
  fireEvent.change(search, { target: { value: '' } }); expect(keys()).toEqual(foldedKeys);
  expect(load).not.toHaveBeenCalled(); act(() => localeController.setLocale('en'));
});

it.each(['many', 'initial', 'middle'] as const)('424 compact: bounded fold and exact re-expansion: %s', scenario => {
  const records = scenario === 'many' ? manyStepRecords() : orderedStepRecords(scenario === 'initial');
  const tx = translator('en'); const projection = projectTrajectory(tx, records); const items = trajectoryItems(records);
  const nativeSteps = projection.sections.flatMap(section => section.kind === 'turn' ? section.groups.filter(group => group.kind === 'step').map(group => group.nativeStepId) : []);
  const project = (folded: boolean) => ledgerRows(tx, projection, items, new Set(folded ? ['ordered-turn'] : []), false);
  const expanded = project(false);
  expect(nativeSteps).toHaveLength(scenario === 'many' ? 50 : 4);
  // Fifty native Steps cost no rows: only visible content and the Turn's
  // own opening seat exist.
  expect(expanded.map(row => row.item?.owner_record_id)).toEqual(scenario === 'middle' ? ['trace:702', 'trace:706'] : ['trace:702', 'trace:702', 'trace:706']);
  expect(expanded.filter(row => row.kind === 'structure' && !row.turnStart)).toEqual([]);
  const collapsed = project(true);
  if (scenario === 'middle') {
    expect(collapsed.map(row => row.kind)).toEqual(['semantic', 'summary']);
    expect(collapsed.filter(row => row.turnStart)).toHaveLength(1);
    expect(collapsed.at(-1)!.summary).toBe('4 steps · 0 tool calls');
    expect(collapsed[0]!.item?.owner_record_id).toBe('trace:702');
  } else {
    // As in Harness, a Turn with one content row (the prompt is not content) has nothing to fold.
    expect(collapsed).toEqual(expanded);
  }
  expect(project(false)).toEqual(expanded);
});

it('424 compact: Steps are never arrow targets, folded or expanded, and need no structural reads', () => {
  const load = vi.fn(); show(cacheOf(orderedStepRecords(false)), load);
  foldTurn('Turn 1');
  expect(document.querySelector('[data-step]')).toBeNull();
  const first = row('RecordRow', 'trace:702'); act(() => first.focus());
  fireEvent.keyDown(first, { key: 'ArrowDown' });
  expect(document.activeElement).toBe(first);
  fireEvent.click(turnSummary('ordered-turn')!);
  expect(document.querySelector('[data-step]')).toBeNull();
  act(() => row('RecordRow', 'trace:702').focus());
  fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' }); expect(document.activeElement).toBe(row('RecordRow', 'trace:706'));
  expect(load.mock.calls).toEqual([['trace:706']]);
});

it('424 compact: search exposes folded content and clearing restores exact compact rows', () => {
  const load = vi.fn(); show(cacheOf(orderedStepRecords(false)), load);
  foldTurn('Turn 1');
  const keys = () => [...screen.getByRole('table').querySelectorAll('[data-display-key]')].map(el => el.getAttribute('data-display-key'));
  const collapsedKeys = keys();
  const owners = () => [...screen.getByRole('table').querySelectorAll<HTMLElement>('[role="row"][data-owner]')].map(el => el.dataset.owner);
  expect(document.querySelectorAll('[data-display-type="StructuralSeat"]')).toHaveLength(0);
  expect(owners()).toEqual(['trace:702']);
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: 'order-match later' } });
  expect(owners()).toEqual(['trace:706']);
  expect(document.querySelector('[data-step]')).toBeNull();
  expect(document.querySelectorAll('[data-display-type="TurnSummary"]')).toHaveLength(0);
  fireEvent.change(search, { target: { value: '' } });
  expect(keys()).toEqual(collapsedKeys);
  expect(document.querySelectorAll('[data-display-type="StructuralSeat"]')).toHaveLength(0);
  expect(document.querySelectorAll('[data-display-type="TurnSummary"]')).toHaveLength(1);
  expect(load).not.toHaveBeenCalled();
});

it.each(['structure-only', 'initial-only', 'updated-system'] as const)('424 compact: sparse Turn needs no Step seats: %s', scenario => {
  const records = orderedStepRecords().filter(record => record.kind === 'attempt' || record.kind === 'step' || (scenario !== 'structure-only' && record.kind === 'request'));
  if (scenario === 'updated-system') {
    const updated = traceRecord(707, { location: { attempt_id: 'ordered-turn', step_id: 'b-last' } });
    updated.request!.system_prompt = { state: 'changed', preview: { text: 'Updated prompt', truncated: false } };
    records.push(updated);
  }
  const tx = translator('en');
  const rows = ledgerRows(tx, projectTrajectory(tx, records), trajectoryItems(records), new Set(['ordered-turn']), false);
  // Without content rows there is nothing to fold; the Turn keeps one seat for its label.
  expect(rows.some(row => row.kind === 'summary')).toBe(false);
  expect(rows.filter(row => row.turnStart)).toHaveLength(1);
  expect(rows.filter(row => row.kind === 'semantic').map(row => row.item?.type)).toEqual(scenario === 'structure-only' ? [] : scenario === 'initial-only' ? ['SystemPromptCell'] : ['SystemPromptCell', 'SystemPromptCell']);
  const load = vi.fn(); show(cacheOf(records), load);
  expect(screen.getByLabelText('Turn 1', { selector: 'span' })).toBeDefined();
  expect(load).not.toHaveBeenCalled();
});

it('424 compact: summary counts the folded Turn\'s Tool records, as Harness does', () => {
  const records = orderedStepRecords(false);
  records.at(-1)!.calls = [{ call_id: 'one', tool_id: 'tool-a', name: 'same' }, { call_id: 'two', tool_id: 'tool-a', name: 'same' }];
  records.push(traceTool(708, { location: { attempt_id: 'ordered-turn', step_id: 'b-last' } }));
  const tx = translator('en'); const projection = projectTrajectory(tx, records);
  const rows = ledgerRows(tx, projection, trajectoryItems(records), new Set(['ordered-turn']), false);
  expect(rows.map(row => row.kind)).toEqual(['semantic', 'summary']);
  expect(rows.at(-1)!.summary).toBe('4 steps · 1 tool call');
  expect(rows.some(row => row.item?.owner_record_id === 'trace:706')).toBe(false);
});

it('Agent activations retain separate trace records correlated to one durable Agent across replay', () => {
  const records = ['activation-a', 'activation-b', 'activation-c'].map((activation_id, n) => traceRecord(n, {
    kind: 'subagent', request: null, agent_id: 'durable-agent', activation_id, native_id: activation_id,
    activation_origin: n === 0 ? { kind: 'creation_tool', tool_call_id: 'actual-create-call' } : n === 1 ? { kind: 'message_tool', tool_call_id: 'actual-message-call' } : { kind: 'client_control' },
    originating_tool_call_id: n === 0 ? 'actual-create-call' : n === 1 ? 'actual-message-call' : null,
  }));
  const ui = show(cacheOf(records));
  for (const record of records) {
    fireEvent.click(row('RecordRow', record.id));
    expect(row('RecordRow', record.id).getAttribute('aria-selected')).toBe('true');
    expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Summary', 'Preview', 'Timing']);
  }
  ui.rerender(<Trajectory cache={cacheOf(structuredClone(records))} loadEarlier={noop} onSelect={noop} onLoadDetail={noop}/>);
  expect(document.querySelectorAll('[data-display-type="RecordRow"]')).toHaveLength(3);
});


it('424: explicit-null Step membership preserves native order across partial windows, prepends, folding and search', () => {
  const records = stepLessRecords();
  for (const window of [records.slice(1), records]) {
    const projection = projectTrajectory(translator('en'), window);
    const items = trajectoryItems(window);
    const expanded = ledgerRows(translator('en'), projection, items, new Set(), false);
    expect(expanded.flatMap(row => row.item ? [row.item.owner_record_id] : [])).toEqual(window.map(record => record.id));
    const userTargets = ledgerFocusTargets(expanded).filter(target => target.item.type !== 'RequestBoundary');
    expect(userTargets.map(target => isInspectable(target.item) && target.item.owner_record_id)).toEqual(window.map(record => record.id));
    const folded = new Set(['adopted-attempt']);
    const collapsed = ledgerRows(translator('en'), projection, items, folded, false);
    expect(collapsed.flatMap(row => row.item ? [row.item.owner_record_id] : [])).toEqual([window[0]!.id]);
    expect(collapsed.map(row => row.height)).toEqual([30, 20]);
    const matches = searchItems(projection, window[0]!.preview!.text)!;
    const searched = ledgerRows(translator('en'), projection, visibleItems(translator('en'), items, window, new Set(), matches), folded, true);
    expect(searched.flatMap(row => row.item ? [row.item.owner_record_id] : [])).toEqual([window[0]!.id]);
    expect(ledgerRows(translator('en'), projection, items, folded, false)).toEqual(collapsed);
  }
  const partial = replaceTrace({ records: records.slice(1), next_cursor: 'older' });
  const prepended = prependTrace(partial, { records: records.slice(0, 2), next_cursor: null });
  expect(prepended.page.records.map(record => record.id)).toEqual(records.map(record => record.id));
});

it('424: wire-null User survives folds and search, is keyboard reachable and selects only its exact Inspector owner', () => {
  const records = stepLessRecords();
  const load = vi.fn(); const older = vi.fn(); const select = vi.fn();
  const ui = render(<Trajectory cache={cacheOf(records.slice(1))} loadEarlier={older} onSelect={select} onLoadDetail={load}/>);
  ui.rerender(<Trajectory cache={cacheOf(records)} loadEarlier={older} onSelect={select} onLoadDetail={load}/>);
  for (const record of records) expect(document.querySelectorAll(`[data-display-type="RecordRow"][data-owner="${record.id}"]`)).toHaveLength(1);
  foldTurn('Turn 1');
  expect(row('RecordRow', records[0]!.id)).not.toBeNull();
  expect(row('RecordRow', records[1]!.id)).toBeNull();
  expect(row('RecordRow', records[2]!.id)).toBeNull();
  const search = screen.getByRole('searchbox', { name: 'Search trajectory' });
  fireEvent.change(search, { target: { value: 'adopted second' } });
  expect(row('RecordRow', records[1]!.id)).not.toBeNull();
  fireEvent.change(search, { target: { value: '' } });
  expect(row('RecordRow', records[0]!.id)).not.toBeNull();
  expect(row('RecordRow', records[1]!.id)).toBeNull();
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
  act(() => row('RecordRow', records[0]!.id).focus());
  fireEvent.keyDown(row('RecordRow', records[0]!.id), { key: 'Enter' });
  expect(select).toHaveBeenLastCalledWith(records[0]!.id);
  expect(load.mock.calls).toEqual([[records[0]!.id]]);
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Summary', 'Preview', 'Raw', 'Source']);
  expect(row('RecordRow', records[0]!.id).getAttribute('aria-selected')).toBe('true');
});

it('retained idle inputs move into their native answering turns, below the initial prompt, when refreshed outside the latest page', () => {
  const first = traceRecord(0, { kind: 'user', request: null, location: {}, preview: { text: 'first input', truncated: false } });
  const request = richRequest(2);
  request.request!.context_additions = [];
  const second = traceRecord(4, { kind: 'user', request: null, location: {}, preview: { text: 'second input', truncated: false } });
  const records = [first, request,
    traceRecord(3, { kind: 'assistant', request: null }), second,
    traceRecord(5, { kind: 'assistant', request: null, location: { attempt_id: 'attempt-b', step_id: '1' } })];
  const before = selectTrace(cacheOf(records), first.id);
  const updates = [first, second].map((record, i) => ({ id: record.id,
    location: { attempt_id: i ? 'attempt-b' : 'attempt-a', step_id: null },
    state: record.state, timing: record.timing, attachments: [], truncated: false }));
  const repaired = refreshTrace(before, { records: [records.at(-1)!] }, updates);
  expect(repaired.selection?.location).toEqual(updates[0].location);
  const projection = projectTrajectory(translator('en'), repaired.page.records);
  const rows = ledgerRows(translator('en'), projection, trajectoryItems(repaired.page.records), new Set(), false);
  expect(rows.filter(row => row.kind === 'semantic').map(row => [row.item!.type, row.item!.owner_record_id, row.turnStart])).toEqual([
    ['SystemPromptCell', request.id, false], ['RecordRow', first.id, true],
    ['RecordRow', 'trace:3', false], ['RecordRow', second.id, true], ['RecordRow', 'trace:5', false],
  ]);
  expect(rows.filter(row => row.turnStart).map(row => [row.item!.owner_record_id, row.turn!.attempt_id])).toEqual([
    [first.id, 'attempt-a'], [second.id, 'attempt-b'],
  ]);
  const load = vi.fn(); show({ ...repaired, selection: undefined }, load);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect([...ledger.querySelectorAll('[data-display-type="SystemPromptCell"], [data-display-type="RecordRow"]')].length).toBeGreaterThan(0);
  expect(load).not.toHaveBeenCalled();
});

it('system prompt and context timeline inputs select their own facets, not the shared request boundary', () => {
  const request = richRequest();
  const detail = requestDetail(0); detail.request!.effective_system_prompt = { text: '# Frozen system prompt\n\nHistorical instructions.', truncated: false };
  show(completeTraceDetail(cacheOf([request]), request.id, 1, detail));
  fireEvent.click(row('SystemPromptCell'));
  const span = (kind: string) => document.querySelector<HTMLElement>(`[data-timeline-span="${kind}"]`)!;
  expect(span('system').getAttribute('data-current')).toBe('true'); expect(span('request').hasAttribute('data-current')).toBe(false);
  expect(screen.getByRole('heading', { name: 'Frozen system prompt' })).toBeDefined();
  pressSpan(span('request').dataset.recordId!);
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
  expect(row('SystemPromptCell').getAttribute('aria-selected')).not.toBe('true');
  pressSpan(span('system').dataset.recordId!);
  expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
  pressSpan(span('context').dataset.recordId!);
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
});

it('distinguishes an absent historical prompt from pending and failed reads', () => {
  const request = richRequest(), detail = requestDetail(0);
  const view = show(cacheOf([request])); fireEvent.click(row('SystemPromptCell'));
  expect(screen.queryByText('No system prompt in this request')).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('Loading');
  const renderCache = (cache: TraceCache) => view.rerender(<Trajectory cache={cache} loadEarlier={noop} onSelect={noop} onLoadDetail={noop}/>);
  renderCache(completeTraceDetail(cacheOf([request]), request.id, 1, undefined, 'Historical read failed'));
  expect(screen.getByRole('alert').textContent).toContain('Historical read failed');
  expect(screen.queryByText('No system prompt in this request')).toBeNull();
  detail.request!.effective_system_prompt = { text: '', truncated: false };
  renderCache(completeTraceDetail(cacheOf([request]), request.id, 1, detail));
  expect(screen.getByText('No system prompt in this request')).toBeDefined();
});

it('Harness message details show semantic preview and raw text, never message JSON', () => {
  const record = traceRecord(0, { kind: 'assistant', request: null });
  const detail = requestDetail(0, { kind: 'assistant', request: null, messages: [] });
  detail.messages = [{ message_id: 'assistant-0', role: 'assistant', truncated: false, blocks: [{ type: 'reasoning', text: { text: 'Private reasoning text', truncated: false } }, { type: 'text', text: { text: '# Answer\n\n**Bold content**', truncated: false } }] }];
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(row('RecordRow'));
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Summary', 'Preview', 'Raw']);
  fireEvent.click(screen.getByRole('tab', { name: 'Preview' }));
  expect(screen.getByRole('heading', { name: 'Answer' })).toBeTruthy();
  // As in Harness, recorded thinking opens expanded above the answer.
  expect(screen.getByRole('button', { name: 'Thinking' }).getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByText('Private reasoning text')).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Raw' }));
  expect(screen.queryByRole('heading', { name: 'Answer' })).toBeNull();
  expect(screen.getByRole('tabpanel').textContent).toContain('# Answer\n\n**Bold content**');
  expect(screen.getByRole('tabpanel').textContent).not.toContain('message_id');
});

it('Harness context details restrict preview and raw content to the selected frozen message', () => {
  const record = richRequest(); record.state = 'failed'; const detail = requestDetail(0);
  const base = detail.request!.messages[0]!;
  detail.request!.messages = ['context-z', 'context-a'].map(message_id => ({ ...base, message_id, blocks: [{ type: 'text', text: { text: `# ${message_id}`, truncated: false } }] }));
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(document.querySelector('[data-display-type="ContextRow"][data-display-key*="context-a"]')!);
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Summary', 'Preview', 'Raw', 'Source']);
  expect(screen.getByRole('tabpanel').textContent).not.toContain('context-z');
  // An admitted Context fact is complete even when its Request later failed.
  expect(screen.getByRole('tabpanel').textContent).not.toContain('Failed');
  fireEvent.click(screen.getByRole('tab', { name: 'Preview' }));
  expect(screen.getByRole('heading', { name: 'context-a' })).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Raw' }));
  expect(screen.getByRole('tabpanel').textContent).toContain('# context-a');
  expect(screen.getByRole('tabpanel').textContent).not.toContain('context-z');
});


it('tool result attachments remain available in summary and result without an extra facet', () => {
  const record = traceTool(1); const detail = toolDetail(1);
  detail.tool!.result!.attachments = [{ artifact_id: 'output-file', name: 'report.md', mime_type: 'text/markdown', image: false }];
  show(completeTraceDetail(cacheOf([record]), record.id, 1, detail));
  fireEvent.click(row('RecordRow', record.id));
  expect(screen.queryByRole('tab', { name: 'Artifacts' })).toBeNull();
  expect(within(screen.getByRole('tabpanel')).getByText('report.md')).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Result' }));
  expect(within(screen.getByRole('tabpanel')).getByText('report.md')).toBeTruthy();
});
