import { translator } from '../src/locale/translation';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted interaction contracts; see PROVENANCE.md. */
import { timelineFocus, timelineProjectionRevision, trajectoryTimeline } from '../src/app/trajectory/timeline';
import { useCallback, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { prependTrace, beginTraceDetail, completeTraceDetail, refreshTrace, replaceTrace, selectTrace, type TraceCache } from '../src/client/trace';
import { matchedRecordIds, isInspectable, projectTrajectory, trajectoryItems as flattenTrajectory, visibleItems, matchingCalls, preferredItem, preferredStructure, systemPresentation, type InspectableDisplayItem } from '../src/app/trajectory/layout';
import { searchItems } from '../src/app/trajectory/search';
import { structuralSearchRecords, requestDetail, toolDetail, traceRecord, traceTool } from './trace-fixture';
import type { TraceContextPresentation, TraceDetail, TraceRecord } from '../../protocol/app-server/v23';

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
const structureInspector = () => screen.getByRole('complementary', { name: 'Trace structure inspector' });
/** The value beside one fact label of the structural inspector. */
const fact = (label: string) => within(structureInspector()).getByText(label, { selector: 'dt' }).nextElementSibling?.textContent;
/** The current Timeline canvas, measured as a fixed 100px so clientX is a percent. */
function timelineCanvas() {
  const canvas = screen.getByLabelText('Timeline navigation: arrow keys pan, Escape clears focus');
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

// v23 exposes two independent enums: cover their full Cartesian product,
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
  ['unchanged', 'previous_unavailable', 'Previous Tool catalog unavailable', 'Summary'],
  ['previous_unavailable', 'initial', 'Previous System Prompt unavailable · Initial Tools', 'Tools'],
  ['previous_unavailable', 'changed', 'Previous System Prompt unavailable · Tools Updated', 'Tools'],
  ['previous_unavailable', 'unchanged', 'Previous System Prompt unavailable', 'Summary'],
  ['previous_unavailable', 'previous_unavailable', 'Previous System Prompt unavailable · Previous Tool catalog unavailable', 'Summary'],
] as const;

it.each(inputMatrix)('T1-01 preserves native %s + %s without classification reads', (prompt, tools, label, facet) => {
  const record = richRequest();
  record.request!.system_prompt = { state: prompt, preview: { text: 'Identical bounded preview', truncated: true } };
  record.request!.tool_catalog = tools;
  expect(systemPresentation(translator('en'), record)).toEqual(label ? { label, facet } : undefined);
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
  expect(screen.getByRole('tab', { name: 'Tools' })).toBeDefined();
  expect(load.mock.calls).toEqual([[record.id]]); // only the selected immutable owner
  fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
  expect(screen.getByRole('tabpanel').textContent).toContain(tools.replaceAll('_', ' '));
  fireEvent.click(row('RequestBoundary'));
  expect(screen.queryByRole('tab', { name: 'Diff' }) !== null).toBe(prompt === 'changed');
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
    expect(screen.getByLabelText('System prompt diff').textContent).toContain('Previous prompt.');
    expect(screen.getByLabelText('System prompt diff').textContent).toContain('You are the historical agent.');
  } else {
    expect(screen.queryByRole('tab', { name: 'Diff' })).toBeNull();
    expect(screen.queryByLabelText('System prompt diff')).toBeNull();
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
  fireEvent.click(row('SystemPromptCell'));
  fireEvent.click(row('RequestBoundary', 'trace:1'));
  fireEvent.click(row('ContextRow', 'trace:1'));
  const selected = row('ContextRow', 'trace:1'); selected.focus();
  expect(reads).toEqual(['trace:0', 'trace:1']);
  await act(async () => { resolvers.get('trace:1')!(requestDetail(1)); });
  await act(async () => { resolvers.get('trace:0')!(requestDetail(0)); });
  expect(screen.getByRole('tab', { name: 'Context' }).getAttribute('aria-selected')).toBe('true');
  expect(selected.getAttribute('aria-selected')).toBe('true');
  expect(document.activeElement).toBe(selected);
  fireEvent.click(row('SystemPromptCell', 'trace:1'));
  fireEvent.click(row('RequestBoundary', 'trace:1'));
  expect(reads).toEqual(['trace:0', 'trace:1']);
  expect(screen.getByRole('tab', { name: 'Summary' }).getAttribute('aria-selected')).toBe('true');
});

it('T1-05 latest unchanged-only page directly discovers prompt with one lazy owner read', () => {
  const load = vi.fn(); show(cacheOf([traceRecord(9), traceRecord(10)]), load);
  expect(load).not.toHaveBeenCalled(); expect(document.querySelector('[data-display-type="SystemPromptCell"]')).toBeNull();
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
  fireEvent.click(row('SystemPromptCell')); fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
  expect(screen.getByText(/A complete diff cannot be produced:/)).toBeDefined();
  expect(screen.queryByText(/No changes/)).toBeNull();
});

it('T1-02 diff distinguishes initial, unavailable, empty and complete changed content', () => {
  const record = richRequest(); record.request!.system_prompt.state = 'changed';
  const detail = requestDetail(0); detail.request!.previous_system_prompt = { text: '', truncated: false }; detail.request!.effective_system_prompt = { text: 'new prompt\n', truncated: false };
  const cache = completeTraceDetail(cacheOf([record]), record.id, 1, detail);
  const view = show(cache); fireEvent.click(row('SystemPromptCell')); fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
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
  expect(items.filter(item => item.type === 'GroupHeader')).toHaveLength(1);
  expect(items.filter(item => item.type === 'RequestBoundary').map(item => item.record.request!.retry_number)).toEqual([0, 1, 2, 3]);
  show(cacheOf(records));
  for (const record of records) { fireEvent.click(row('RequestBoundary', record.id)); expect(row('RequestBoundary', record.id).getAttribute('aria-selected')).toBe('true'); }
  expect(screen.queryByText('Fold Steps')).toBeNull();
});

it('T1-06/10 segment anchors regroup within the same structure without acquiring detail ownership', () => {
  const middle = traceRecord(10);
  const segment = trajectoryItems([middle]).find(item => item.type === 'GroupHeader')!;
  expect(segment.anchor_record_id).toBe(middle.id);
  expect('owner_record_id' in segment).toBe(false);
  const nativeStep = traceRecord(8, { kind: 'step', request: null });
  const all = trajectoryItems([nativeStep, traceRecord(9), middle]);
  const target = preferredStructure(all, segment)!;
  expect(target.type).toBe('GroupHeader');
  expect(target.native_record?.id).toBe(nativeStep.id);
  expect(target.anchor_record_id).toBe(nativeStep.id);
  expect('owner_record_id' in target).toBe(false);
  const split = trajectoryItems([traceRecord(9), traceRecord(11, { kind: 'background', request: null, location: {} }), middle]);
  expect(split.filter(item => item.type === 'GroupHeader')).toHaveLength(1);
  expect(preferredStructure(split, segment)?.anchor_record_id).toBe('trace:9');
  expect(preferredStructure(trajectoryItems([traceRecord(9)]), segment)?.display_key).toBe(segment.display_key);
  expect(split.filter(item => item.type === 'RecordRow' || item.type === 'RequestBoundary').map(item => item.owner_record_id)).toEqual(['trace:9', 'trace:10', 'trace:11']);
});

it.each(['request', 'tool'] as const)('T1-04/06 mid-Step %s anchor never lends its evidence to a structure without its exact native record', kind => {
  const child = kind === 'request' ? traceRecord(10) : traceTool(10);
  const load = vi.fn(); const select = vi.fn(); const older = vi.fn();
  render(<Trajectory cache={cacheOf([child])} onSelect={select} onLoadDetail={load} loadEarlier={older} latest={noop} />);
  const items = trajectoryItems([child]);
  const step = items.find(item => item.type === 'GroupHeader')!;
  expect(step.display_key).toBe(JSON.stringify(['group', 'attempt-a', '1']));
  expect(trajectoryItems([{ ...child, state: 'running' }]).find(item => item.type === 'GroupHeader')!.display_key).toBe(step.display_key);
  for (const type of ['TurnHeader', 'GroupHeader']) {
    const header = document.querySelector<HTMLElement>(`[data-display-type="${type}"]`)!;
    act(() => header.focus()); fireEvent.click(header); fireEvent.keyDown(header, { key: 'Enter' });
    expect(header.hasAttribute('data-owner')).toBe(false);
    expect(document.activeElement).toBe(header);
    expect(screen.queryByRole('complementary', { name: 'Trace record inspector' })).toBeNull();
    const evidence = structureInspector().textContent!;
    expect(evidence).toContain(`The exact native ${type === 'TurnHeader' ? 'Attempt' : 'Step'} record is not loaded at this read cut`);
    for (const borrowed of [child.id, 'attempt-a', child.state, 'request-10', 'tool-bash']) expect(evidence).not.toContain(borrowed);
    expect(within(structureInspector()).queryByText('Record')).toBeNull();
  }
  expect(select.mock.calls.every(([id]) => id === undefined)).toBe(true);
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
  fireEvent.click(row(kind === 'request' ? 'RequestBoundary' : 'RecordRow', child.id));
  expect(select).toHaveBeenLastCalledWith(child.id);
  expect(load.mock.calls).toEqual([[child.id]]);
  expect(screen.getByRole('tab', { name: kind === 'request' ? 'System Prompt' : 'Input' })).toBeDefined();
});

it('407: exact native Attempt and Step records are inspectable structural evidence without any detail read', () => {
  const location = { attempt_id: 'attempt-opaque', step_id: 'step-opaque' };
  // The Attempt has not settled and the Step was cancelled before any Request.
  const attempt = traceRecord(8, { kind: 'attempt', request: null, preview: null, location: { attempt_id: location.attempt_id }, state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } });
  const input = traceRecord(9, { kind: 'user', request: null, location: { attempt_id: location.attempt_id }, preview: { text: 'attempt input', truncated: false } });
  const step = traceRecord(10, { kind: 'step', request: null, preview: null, location, state: 'cancelled' });
  const records = [attempt, input, step];
  const structures = trajectoryItems(records).filter(item => item.type === 'GroupHeader' || item.type === 'TurnHeader');
  expect(structures.map(item => [item.label, item.native_record?.id])).toEqual([['Turn 1', attempt.id], ['Message', undefined], ['Step 1', step.id]]);
  const load = vi.fn(); const select = vi.fn();
  render(<Trajectory cache={cacheOf(records)} onSelect={select} onLoadDetail={load} loadEarlier={noop} latest={noop} />);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect(ledger.textContent).not.toContain('attempt-opaque');
  expect(ledger.textContent).not.toContain('step-opaque');
  expect(ledger.textContent).not.toContain('Attempt');
  const turn = within(ledger).getByRole('row', { name: 'Turn 1' });
  const message = within(ledger).getByRole('row', { name: 'Message' });
  const stepRow = within(ledger).getByRole('row', { name: 'Step 1' });

  fireEvent.click(turn);
  expect(screen.queryByRole('complementary', { name: 'Trace record inspector' })).toBeNull();
  expect(within(structureInspector()).getByText('Turn 1 · native Attempt')).toBeDefined();
  expect(fact('Native kind')).toBe('Attempt');
  expect(fact('Record')).toBe(attempt.id);
  expect(fact('Attempt')).toBe('attempt-opaque');
  expect(fact('State')).toBe('running');
  expect(fact('Ended')).toBe('Unavailable');
  expect(fact('Duration')).toBe('Unavailable');
  expect(within(structureInspector()).queryByText('Logical Step')).toBeNull();

  // Keyboard: arrows move structural selection, Enter/Space open, Escape closes in place.
  act(() => turn.focus()); fireEvent.keyDown(turn, { key: 'ArrowDown' });
  expect(document.activeElement).toBe(message);
  expect(structureInspector().textContent).toContain('no native structural record');
  expect(structureInspector().textContent).not.toContain(input.id);
  expect(structureInspector().textContent).not.toContain('attempt input');
  act(() => stepRow.focus()); fireEvent.keyDown(stepRow, { key: 'Enter' });
  expect(stepRow.getAttribute('aria-selected')).toBe('true');
  expect(within(structureInspector()).getByText('Step 1 · native Step')).toBeDefined();
  expect(fact('Native kind')).toBe('Step');
  expect(fact('Record')).toBe(step.id);
  expect(fact('Attempt')).toBe('attempt-opaque');
  expect(fact('Logical Step')).toBe('step-opaque');
  expect(fact('State')).toBe('cancelled');
  expect(fact('Duration')).toBe('1.00 s');
  fireEvent.keyDown(stepRow, { key: 'Escape' });
  expect(screen.queryByRole('complementary')).toBeNull();
  expect(document.activeElement).toBe(stepRow);
  fireEvent.keyDown(stepRow, { key: ' ' });
  const close = within(structureInspector()).getByRole('button', { name: 'Close structure' });
  act(() => close.focus()); fireEvent.click(close);
  // Focus returns to the header without reopening its inspection.
  expect(document.activeElement).toBe(stepRow);
  expect(screen.queryByRole('complementary')).toBeNull();
  expect(load).not.toHaveBeenCalled();
  expect(select.mock.calls.every(([id]) => id === undefined)).toBe(true);
  expect(document.querySelector('[data-display-type="TurnHeader"]')?.textContent).toContain('running');
});

it('407: prepend renumbering and lifecycle refresh keep the same native structural selection', () => {
  const attempt = traceRecord(8, { kind: 'attempt', request: null, preview: null, location: { attempt_id: 'attempt-a' }, state: 'running' });
  const records = [attempt, traceRecord(9, { kind: 'step', request: null, preview: null }), traceRecord(10)];
  const initial = cacheOf(records);
  const load = vi.fn(); const view = show(initial, load);
  const turn = screen.getByRole('row', { name: 'Turn 1' });
  act(() => turn.focus()); fireEvent.keyDown(turn, { key: 'Enter' });
  expect(fact('Record')).toBe(attempt.id);
  const older = prependTrace(initial, { records: [traceRecord(1, { location: { attempt_id: 'older-attempt', step_id: 'older-step' } })], next_cursor: null });
  view.rerender(<Trajectory cache={older} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={load} />);
  expect(screen.getByRole('row', { name: 'Turn 2' })).toBe(turn);
  expect(turn.getAttribute('aria-selected')).toBe('true');
  expect(document.activeElement).toBe(turn);
  expect(within(structureInspector()).getByText('Turn 2 · native Attempt')).toBeDefined();
  expect(fact('Record')).toBe(attempt.id);
  expect(fact('Attempt')).toBe('attempt-a');
  expect(fact('State')).toBe('running');
  const refreshed = refreshTrace(older, { records: [{ ...attempt, state: 'completed' }], next_cursor: null });
  expect(refreshed.epoch).toBe(initial.epoch);
  view.rerender(<Trajectory cache={refreshed} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={load} />);
  expect(fact('Record')).toBe(attempt.id);
  expect(fact('State')).toBe('completed');
  expect(turn.getAttribute('aria-selected')).toBe('true');
  expect(load).not.toHaveBeenCalled();
});

it.each(['request', 'tool'] as const)('T1-04 controlled late %s detail cannot hijack structural focus', async kind => {
  const child = kind === 'request' ? traceRecord(10) : traceTool(10);
  let resolve!: (detail: TraceDetail) => void;
  const reads: string[] = []; const selections: (string | undefined)[] = [];
  function Fixture() {
    const [cache, setCache] = useState(cacheOf([traceRecord(9, { kind: 'step', request: null, preview: null }), child]));
    const load = useCallback((id: string) => {
      reads.push(id); setCache(current => beginTraceDetail(current, id));
      void new Promise<TraceDetail>(done => { resolve = done; }).then(detail => setCache(current => completeTraceDetail(current, id, 1, detail)));
    }, []);
    return <Trajectory cache={cache} onSelect={id => { selections.push(id); setCache(current => selectTrace(current, id)); }} onLoadDetail={load} loadEarlier={noop} latest={noop} />;
  }
  render(<Fixture />);
  fireEvent.click(row(kind === 'request' ? 'RequestBoundary' : 'RecordRow', child.id));
  const step = document.querySelector<HTMLElement>('[data-display-type="GroupHeader"]')!;
  act(() => step.focus()); fireEvent.keyDown(step, { key: 'Enter' });
  expect(selections.at(-1)).toBeUndefined();
  expect(fact('Record')).toBe('trace:9');
  await act(async () => resolve(kind === 'request' ? requestDetail(10) : toolDetail(10)));
  expect(document.activeElement).toBe(step); expect(step.getAttribute('data-selected')).toBe('true');
  expect(screen.queryByRole('complementary', { name: 'Trace record inspector' })).toBeNull();
  expect(fact('Record')).toBe('trace:9'); expect(fact('Logical Step')).toBe('1');
  expect(reads).toEqual([child.id]);
});

const proposal = () => traceRecord(0, { kind: 'assistant', request: null, message_id: 'assistant-0', calls: [{ call_id: 'same', tool_id: 'tool-a', name: 'same name' }, { call_id: 'not-executed', tool_id: 'tool-a', name: 'same name' }] });
const execution = (n: number, overrides: Partial<TraceRecord> = {}) => traceTool(n, { tool: { ...traceTool(n).tool!, call_id: 'same', tool_id: 'tool-a', name: 'same name' }, ...overrides });
it('T1-07 exact scope isolates reused call IDs across Step/Attempt/Tool and page split', () => {
  const assistant = proposal();
  const unrelated = [execution(2, { location: { attempt_id: 'other', step_id: '1' } }), execution(3, { location: { attempt_id: 'attempt-a', step_id: '2' } }), execution(4, { tool: { ...execution(4).tool!, tool_id: 'tool-b' } }), execution(5, { location: {} })];
  expect(matchingCalls([assistant, execution(1), ...unrelated]).get(assistant.id)?.map(r => r.id)).toEqual(['trace:1']);
  const records = [assistant, execution(1), ...unrelated];
  const visible = visibleItems(translator('en'), trajectoryItems(records), records, new Set(), new Set([assistant.id]), null);
  const summary = visible.find(item => item.type === 'CollapsedCallSummary')!;
  expect(summary.preview).toContain('2 proposed · 1 loaded matching executions');
  expect(visible.filter(item => item.type === 'RecordRow').map(item => item.record.id)).toEqual(['trace:0', 'trace:4', 'trace:3', 'trace:2', 'trace:5']);
  expect(matchingCalls([execution(1)]).size).toBe(0);
  expect(matchingCalls([assistant]).size).toBe(0);
  expect(matchingCalls([assistant, { ...assistant, id: 'other-proposer' }, execution(1)]).size).toBe(0);
});

it('T1-08 Calls summary exposes warnings and leaves native domains independent', () => {
  const assistant = proposal();
  const executions = ['failed', 'denied', 'waiting', 'outcome_unknown'].map((state, n) => execution(n + 1, { state: state as TraceRecord['state'] }));
  const domains = ['background', 'subagent', 'workflow'].map((kind, n) => traceRecord(n + 10, { kind: kind as TraceRecord['kind'], request: null, originating_tool_call_id: 'same' }));
  const records = [assistant, ...executions, ...domains, traceRecord(20, { kind: 'compaction', request: null, state: 'running' })];
  const visible = visibleItems(translator('en'), trajectoryItems(records), records, new Set(), new Set([assistant.id]), null);
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
  fireEvent.click(screen.getByRole('button', { name: 'Fold Turns' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Search loaded Trace' }), { target: { value: 'ls -la' } });
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
  expect(document.querySelector('[data-record-id="trace:0"]')!.getAttribute('data-marker')).toBe('true');
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
  const matches = searchItems(projectTrajectory(translator('en'), [child, other]), 'unique child preview')!;
  expect(items.filter(item => matches.has(item.display_key)).every(item => item.type !== 'GroupHeader' && item.type !== 'TurnHeader')).toBe(true);
  const structural = items.find(item => item.type === 'GroupHeader')!;
  expect(searchItems(projectTrajectory(translator('en'), [child, other]), 'Step 1')).toEqual(new Set(items.filter(isInspectable).map(item => item.display_key)));
  expect(preferredStructure(trajectoryItems([{ ...child, location: { attempt_id: 'attempt-b', step_id: '1' } }]), structural)).toBeUndefined();
  const visible = visibleItems(translator('en'), items, [child, other], new Set(['attempt-a']), new Set(), null);
  expect(visible.filter(item => item.type === 'RequestBoundary')).toHaveLength(0);
  expect(visible.filter(item => item.type === 'RecordRow').map(item => item.owner_record_id)).toEqual([other.id]);
  expect(visible.filter(item => item.type === 'GroupHeader').map(item => item.attempt_id)).toEqual(['attempt-b']);
});

it('T1-04 timeline navigation explicitly selects its native Request after structural focus', () => {
  const child = traceRecord(10); const load = vi.fn(); show(cacheOf([child]), load);
  const header = document.querySelector<HTMLElement>('[data-display-type="GroupHeader"]')!;
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
  const folded = visibleItems(translator('en'), items, records, new Set(['attempt-a', 'attempt-b']), new Set(), null);
  expect(folded.filter(isInspectable).map(item => item.owner_record_id)).toEqual(['trace:0']);
  expect(trajectoryTimeline(translator('en'), projection, 'sequence')!.boundaries.map(boundary => [boundary.nativeAttemptId, boundary.label])).toEqual(
    items.filter(item => item.type === 'TurnHeader').map(item => [item.attempt_id, item.label]),
  );
  show(cacheOf(records));
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect(ledger.textContent).not.toContain('opaque-step');
  expect(ledger.textContent).not.toContain('attempt-a');
  fireEvent.click(row('RequestBoundary', 'trace:2'));
  fireEvent.click(screen.getByRole('tab', { name: 'Native' }));
  expect(screen.getByRole('tabpanel').textContent).toContain('opaque-step');
  expect(screen.getByRole('tabpanel').textContent).toContain('attempt-a');
  expect(screen.getByRole('tabpanel').textContent).toContain('request-2');
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
  const collapse = screen.getByRole('button', { name: 'Fold Turn 1' });
  act(() => collapse.focus());
  fireEvent.click(collapse);
  expect(screen.getByRole('complementary').textContent).toContain('Exact frozen request ten');
  expect(row('SystemPromptCell', selected.id)).toBeNull();
  const next = prependTrace(initial, { records: [older], next_cursor: null });
  view.rerender(<Trajectory cache={next} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={load} />);
  expect(screen.getByRole('button', { name: 'Expand Turn 2' })).toBe(collapse);
  expect(document.activeElement).toBe(collapse);
  expect(screen.getByRole('complementary').textContent).toContain('Exact frozen request ten');
  expect(row('SystemPromptCell', selected.id)).toBeNull();
  const search = screen.getByRole('textbox', { name: 'Search loaded Trace' });
  fireEvent.change(search, { target: { value: 'request-10' } });
  expect(row('SystemPromptCell', selected.id).getAttribute('aria-selected')).toBe('true');
  expect(screen.getByRole('row', { name: 'Turn 2' })).toBeDefined();
  fireEvent.change(search, { target: { value: 'request-1' } });
  fireEvent.change(search, { target: { value: '' } });
  expect(row('SystemPromptCell', selected.id)).toBeNull();
  expect(screen.getByRole('button', { name: 'Expand Turn 2' })).toBeDefined();
  fireEvent.click(screen.getByRole('button', { name: 'Expand Turn 2' }));
  expect(row('SystemPromptCell', selected.id).getAttribute('aria-selected')).toBe('true');
  fireEvent.click(screen.getByRole('button', { name: 'Close record' }));
  expect(document.activeElement).toBe(row('SystemPromptCell', selected.id));
  expect(load).not.toHaveBeenCalled();
});

it('407: System Prompt cells expose semantic tabs and preserve unknown historical evidence', () => {
  const initial = richRequest();
  const view = show(completeTraceDetail(cacheOf([initial]), initial.id, 1, requestDetail(0)));
  fireEvent.click(row('SystemPromptCell'));
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['System Prompt', 'Tools', 'Summary', 'Native']);
  expect(screen.queryByRole('tab', { name: 'Diff' })).toBeNull();
  const unknown = richRequest();
  unknown.request!.system_prompt.state = 'previous_unavailable';
  unknown.request!.tool_catalog = 'previous_unavailable';
  const detail = requestDetail(0);
  detail.request!.previous_system_prompt = null;
  detail.request!.predecessor = { availability: 'unavailable', request_id: 'historical-predecessor' };
  view.rerender(<Trajectory cache={completeTraceDetail(cacheOf([unknown]), unknown.id, 1, detail)} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  fireEvent.click(row('SystemPromptCell'));
  expect(screen.queryByRole('tab', { name: 'Diff' })).toBeNull();
  fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
  expect(screen.getByRole('tabpanel').textContent).toContain('previous unavailable');
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
  const before = visibleItems(translator('en'), items, records, collapsed, new Set(), null);
  const matches = searchItems(projectTrajectory(translator('en'), records), query);
  expect(matchedRecordIds(items, matches)).toEqual(new Set(ids));
  const exposed = visibleItems(translator('en'), items, records, collapsed, new Set(), matches);
  expect(exposed.filter(isInspectable).map(item => item.owner_record_id)).toEqual([...ids]);
  expect(exposed).toContainEqual(expect.objectContaining({ type, label, attempt_id: attempt }));
  expect(visibleItems(translator('en'), items, records, collapsed, new Set(), null)).toEqual(before);
  expect([...collapsed]).toEqual(['attempt-a', 'attempt-b']);

  const load = vi.fn(); const older = vi.fn();
  show(replaceTrace({ records, next_cursor: 'older' }), load, older);
  fireEvent.click(screen.getByRole('button', { name: 'Fold Turn 1' }));
  fireEvent.click(screen.getByRole('button', { name: 'Fold Turn 2' }));
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  const previousKeys = () => [...ledger.querySelectorAll('[data-display-key]')].map(el => el.getAttribute('data-display-key'));
  const foldedKeys = previousKeys();
  const search = screen.getByRole('textbox', { name: 'Search loaded Trace' });
  fireEvent.change(search, { target: { value: query } });
  expect(within(ledger).getByRole('row', { name: label }).getAttribute('data-attempt')).toBe(attempt);
  expect([...ledger.querySelectorAll('[data-owner]')].map(el => el.getAttribute('data-owner'))).toEqual([...ids]);
  const spans = [...document.querySelectorAll('[data-record-id]')];
  expect(spans.length).toBeGreaterThan(0);
  for (const span of spans) expect(span.hasAttribute('data-dimmed')).toBe(!new Set<string>(ids).has(span.getAttribute('data-record-id')!));
  fireEvent.change(search, { target: { value: '' } });
  expect(previousKeys()).toEqual(foldedKeys);
  expect(screen.getByRole('button', { name: 'Expand Turn 1' })).toBeDefined();
  expect(screen.getByRole('button', { name: 'Expand Turn 2' })).toBeDefined();
  expect(load).not.toHaveBeenCalled();
  expect(older).not.toHaveBeenCalled();
});

it('search conversion covers every inspectable cell, deduplicates owners and excludes history boundaries', () => {
  const assistant = traceRecord(1, { kind: 'assistant', request: null, message_id: 'assistant', calls: [{ call_id: 'call-2', tool_id: 'tool-bash', name: 'bash' }] });
  const records = [richRequest(), assistant, traceTool(2)];
  const items = flattenTrajectory(translator('en'), projectTrajectory(translator('en'), records), 'older');
  const folded = visibleItems(translator('en'), items, records, new Set(), new Set([assistant.id]), null);
  const universe = [...items, ...folded];
  expect(new Set(universe.filter(isInspectable).map(item => item.type))).toEqual(new Set(['SystemPromptCell', 'ContextRow', 'RequestBoundary', 'RecordRow', 'CollapsedCallSummary']));
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
  const search = screen.getByRole('textbox', { name: 'Search loaded Trace' });
  fireEvent.change(search, { target: { value: query } });
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect([...ledger.querySelectorAll('[data-owner]')].map(el => el.getAttribute('data-display-key'))).toEqual([...matches]);
  expect([...document.querySelectorAll('[data-record-id]:not([data-dimmed])')].map(el => el.getAttribute('data-record-id'))).toEqual([owner]);
  expect(screen.getByRole('tab', { name: 'System Prompt' }).getAttribute('aria-selected')).toBe('true');
  fireEvent.change(search, { target: { value: '' } });
  expect(row('SystemPromptCell').getAttribute('aria-selected')).toBe('true');
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
});

it('structural search overrides Calls without changing the stored collapse set', () => {
  const load = vi.fn(); const older = vi.fn();
  show(cacheOf([proposal(), execution(1)]), load, older);
  fireEvent.click(within(screen.getByRole('toolbar')).getByRole('button', { name: 'Collapse Calls' }));
  expect(row('RecordRow', 'trace:1')).toBeNull();
  const search = screen.getByRole('textbox', { name: 'Search loaded Trace' });
  fireEvent.change(search, { target: { value: 'Step 1' } });
  expect(row('RecordRow', 'trace:1')).not.toBeNull();
  expect(row('CollapsedCallSummary')).toBeNull();
  fireEvent.change(search, { target: { value: '' } });
  expect(row('RecordRow', 'trace:1')).toBeNull();
  expect(row('CollapsedCallSummary')).not.toBeNull();
  expect(load).not.toHaveBeenCalled(); expect(older).not.toHaveBeenCalled();
});

it.each(['Turn 2', 'Step 2'])('%s reveals every semantic cell of its structural members', query => {
  const request = richRequest(5); request.location = { attempt_id: 'b', step_id: 'second' };
  const records = [traceRecord(0), traceRecord(1, { location: { attempt_id: 'b', step_id: 'first' } }), request];
  show(cacheOf(records));
  fireEvent.click(screen.getByRole('button', { name: 'Fold Turns' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Search loaded Trace' }), { target: { value: query } });
  const expected = trajectoryItems(query === 'Turn 2' ? records.slice(1) : [request]).filter(isInspectable);
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  expect([...ledger.querySelectorAll('[data-owner]')].map(el => el.getAttribute('data-display-key'))).toEqual(expected.map(cell => cell.display_key));
  expect([...document.querySelectorAll('[data-record-id]:not([data-dimmed])')].map(el => el.getAttribute('data-record-id'))).toEqual(query === 'Turn 2' ? ['trace:1', 'trace:5'] : ['trace:5']);
});

const epochRecords = (...ns: number[]) => ns.map(n => traceRecord(n, { location: { attempt_id: `attempt-${n}`, step_id: 'step' } }));

it('407: Timeline focus keeps native identity across prepend and lifecycle refresh within one Trace epoch', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const view = show(initial);
  expect(timelineFocusOf()).toEqual({ 'trace:0': undefined, 'trace:1': undefined, 'trace:2': undefined, 'trace:3': undefined });
  dragTimeline(30, 45);
  expect(timelineFocusOf()).toEqual({ 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'outside', 'trace:3': 'outside' });
  const before = focusOverlay()!.style.left;
  const canvas = timelineCanvas();
  const prepended = prependTrace(initial, { records: epochRecords(9), next_cursor: null });
  expect(prepended.epoch).toBe(initial.epoch);
  view.rerender(<Trajectory cache={prepended} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  // Same native identities; the Turn ordinal and projected range both moved.
  expect(timelineFocusOf()).toEqual({ 'trace:9': 'outside', 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'outside', 'trace:3': 'outside' });
  expect(screen.getByRole('row', { name: 'Turn 3' }).getAttribute('data-attempt')).toBe('attempt-1');
  expect(focusOverlay()!.style.left).not.toBe(before);
  // Projection changes retire coordinates while committed native focus survives.
  expect(timelineCanvas()).not.toBe(canvas);
  const prependedCanvas = timelineCanvas();
  const refreshed = refreshTrace(prepended, { records: [{ ...prepended.page.records[4]!, state: 'failed' }], next_cursor: null });
  expect(refreshed.epoch).toBe(initial.epoch);
  view.rerender(<Trajectory cache={refreshed} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(timelineFocusOf()).toEqual({ 'trace:9': 'outside', 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'outside', 'trace:3': 'outside' });
  expect(focusOverlay()).not.toBeNull();
  expect(timelineCanvas()).toBe(prependedCanvas);
});

it('407: a Trace epoch rebase retires Timeline focus even when record identities recur', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const view = show(initial);
  dragTimeline(30, 45);
  expect(focusOverlay()).not.toBeNull();
  const replaced = replaceTrace({ records: epochRecords(20, 21, 22, 23), next_cursor: null }, initial);
  expect(replaced.epoch).toBe(initial.epoch + 1);
  view.rerender(<Trajectory cache={replaced} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(focusOverlay()).toBeNull();
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  // The epoch, not identity absence, owns the lifetime: recurring IDs stay unfocused.
  const recurring = replaceTrace({ records: epochRecords(0, 1, 2, 3), next_cursor: null }, replaced);
  view.rerender(<Trajectory cache={recurring} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(focusOverlay()).toBeNull();
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  // A focus created in the new domain belongs to it.
  dragTimeline(55, 70);
  expect(timelineFocusOf()).toEqual({ 'trace:0': 'outside', 'trace:1': 'outside', 'trace:2': 'inside', 'trace:3': 'outside' });
});

it('407: Jump to latest rebases the read domain and leaves no stale Timeline dimming', () => {
  // Mirrors AppServerClient.latestTrace: the resident snapshot replaces the window.
  const snapshot = { records: epochRecords(4, 5, 6, 7), next_cursor: 'older' };
  const epochs: number[] = [];
  function Fixture() {
    const [cache, setCache] = useState(() => replaceTrace(snapshot));
    epochs.push(cache.epoch);
    return <Trajectory cache={cache} onSelect={noop} onLoadDetail={noop} latest={() => setCache(current => replaceTrace(snapshot, current))}
      loadEarlier={() => setCache(current => prependTrace(current, { records: epochRecords(0, 1, 2, 3), next_cursor: null }))} />;
  }
  render(<Fixture />);
  fireEvent.click(screen.getByRole('button', { name: 'Load earlier records' }));
  // Eight equal sequence spans: 13–24% covers only the second, trace:1.
  dragTimeline(13, 24);
  expect(Object.entries(timelineFocusOf()).filter(([, focus]) => focus === 'inside').map(([owner]) => owner)).toEqual(['trace:1']);
  expect(timelineFocusOf()['trace:5']).toBe('outside');
  const ledger = screen.getByRole('table', { name: 'Trace ledger' });
  Object.defineProperty(ledger, 'scrollHeight', { configurable: true, value: 900 });
  ledger.scrollTop = 150; fireEvent.scroll(ledger);
  fireEvent.click(screen.getByRole('button', { name: 'Jump to latest' }));
  expect(epochs.at(-1)).toBe(2);
  expect([...document.querySelectorAll<HTMLElement>('[data-owner]')].map(el => el.dataset.owner)).toEqual(['trace:4', 'trace:5', 'trace:6', 'trace:7']);
  expect(document.querySelectorAll('[data-timeline-focus]')).toHaveLength(0);
  expect(focusOverlay()).toBeNull();
});

const domainOf = () => { const canvas = timelineCanvas(); return [canvas.dataset.domainStart, canvas.dataset.domainEnd]; };

it.each([
  ['unrelated', epochRecords(20, 21, 22, 23)],
  ['recurring', epochRecords(0, 1, 2, 3)],
] as const)('407: an in-flight E1 Timeline drag cannot commit into E2 with %s record IDs and the same numeric domain', (_, next) => {
  const select = vi.fn();
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const render = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={select} onLoadDetail={noop} />;
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
  dragTimeline(30, 45);
  const ids = next.map(record => record.id);
  expect(timelineFocusOf()).toEqual({ [ids[0]!]: 'outside', [ids[1]!]: 'inside', [ids[2]!]: 'outside', [ids[3]!]: 'outside' });
});

it('407: a pressed E1 span cannot select a recurring E2 record after a rebase', () => {
  const select = vi.fn(); const load = vi.fn();
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const render = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={select} onLoadDetail={load} />;
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
  const hint = () => screen.getByLabelText('Timing overview').querySelector(':scope > p')!.textContent;
  const full = domainOf();
  fireEvent.click(screen.getByRole('button', { name: 'Zoom timeline in' }));
  fireEvent.keyDown(timelineCanvas(), { key: 'ArrowRight' });
  const zoomed = domainOf();
  expect(zoomed).not.toEqual(full);
  expect(hint()).toContain('Zoomed');
  fireEvent.focus(document.querySelector('[data-record-id="trace:1"]')!);
  expect(hint()).not.toBe('');
  expect(hint()).not.toContain('Zoomed');
  // E2 reuses the IDs and the numeric domain; only the epoch differs.
  const replaced = replaceTrace({ records: epochRecords(0, 1, 2, 3, 4, 5, 6, 7), next_cursor: null }, initial);
  view.rerender(<Trajectory cache={replaced} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(domainOf()).toEqual(full);
  expect(hint()).toBe('');
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

it.each(['sequence', 'duration', 'actual'] as const)('407: in-flight %s drag cannot cross a same-epoch projection revision', mode => {
  const initialSequence = cacheOf(epochRecords(0, 1, 2, 3));
  const { initial, next } = mode === 'sequence'
    ? { initial: initialSequence, next: prependTrace(initialSequence, { records: epochRecords(9), next_cursor: null }) }
    : timingRevision();
  const p1 = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), initial.page.records), mode)!;
  const p2 = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), next.page.records), mode)!;
  expect(next.epoch).toBe(initial.epoch);
  expect(timelineProjectionRevision(p2, mode)).not.toBe(timelineProjectionRevision(p1, mode));
  if (mode === 'actual') expect([p2.start, p2.end]).toEqual([p1.start, p1.end]);
  const oldRange = mode === 'sequence' ? { start: 1.2, end: 1.8 }
    : { start: p1.start + 3100, end: p1.start + 3200 };
  expect(timelineFocus(p1, oldRange)).not.toEqual(timelineFocus(p2, oldRange));
  const select = vi.fn(); const load = vi.fn();
  const ui = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={select} onLoadDetail={load} />;
  const view = render(ui(initial));
  if (mode !== 'sequence') fireEvent.click(screen.getByRole('button', { name: 'Duration' }));
  if (mode === 'actual') fireEvent.click(screen.getByRole('button', { name: 'Actual time' }));
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
  const from = mode === 'sequence' ? 45 : mode === 'duration' ? 30 : 45;
  const to = mode === 'sequence' ? 55 : mode === 'duration' ? 40 : 55;
  dragTimeline(from, to);
  expect(Object.entries(timelineFocusOf()).filter(([, focus]) => focus === 'inside').map(([id]) => id)).toEqual(['trace:1']);
  expect(select).not.toHaveBeenCalled();
});

it('407: obsolete same-epoch span press and synthesized pointer click cannot open Inspector', () => {
  const initial = cacheOf(epochRecords(0, 1, 2, 3));
  const select = vi.fn(); const load = vi.fn();
  const ui = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={select} onLoadDetail={load} />;
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
  const ui = (cache: TraceCache) => <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />;
  const view = render(ui(initial));
  fireEvent.click(screen.getByRole('button', { name: 'Zoom timeline in' }));
  const stale = timelineCanvas();
  fireEvent.pointerDown(stale, { button: 2, pointerId: 1, clientX: 50 });
  fireEvent.pointerMove(stale, { pointerId: 1, clientX: 40 });
  const next = prependTrace(initial, { records: epochRecords(9), next_cursor: null });
  view.rerender(ui(next));
  expect(domainOf()).toEqual(['0', '9']);
  fireEvent.click(screen.getByRole('button', { name: 'Zoom timeline in' }));
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
  fireEvent.pointerMove(canvas, { pointerId: 1, clientX: 45 });
  const next = refreshTrace(initial, { records: [{ ...initial.page.records[1]!, state: 'failed' }], next_cursor: null });
  expect(timelineProjectionRevision(trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), initial.page.records), 'sequence'), 'sequence'))
    .toBe(timelineProjectionRevision(trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), next.page.records), 'sequence'), 'sequence'));
  view.rerender(<Trajectory cache={next} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(timelineCanvas()).toBe(canvas);
  fireEvent.pointerUp(canvas, { pointerId: 1, clientX: 45 });
  expect(timelineFocusOf()).toEqual({ 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'outside', 'trace:3': 'outside' });
});

it('407: committed native focus survives a timing revision with an identical outer domain', () => {
  const { initial, next } = timingRevision();
  const view = show(initial);
  fireEvent.click(screen.getByRole('button', { name: 'Duration' }));
  fireEvent.click(screen.getByRole('button', { name: 'Actual time' }));
  dragTimeline(45, 55);
  const before = timelineFocusOf();
  expect(before).toEqual({ 'trace:0': 'outside', 'trace:1': 'inside', 'trace:2': 'outside' });
  const width = focusOverlay()!.style.width;
  view.rerender(<Trajectory cache={next} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />);
  expect(timelineFocusOf()).toEqual(before);
  expect(focusOverlay()!.style.width).not.toBe(width);
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
