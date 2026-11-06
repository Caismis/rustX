import { useState } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CheckpointPage, WorkflowTraceNode } from '../../protocol/app-server/v73';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { preferredItem, selectionOf, type InspectableDisplayItem } from '../src/app/trajectory/layout';
import { prependTrace, type TraceCache, type TraceFieldReader } from '../src/client/trace';
import { traceRecord } from './trace-fixture';
import { workflowTraceFixture } from './workflow-trace-fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

function scheduledHistory() {
  const fixture = workflowTraceFixture(73), root = fixture.roots[0];
  root.originating_tool_call_id = null;
  root.location = {};
  for (const node of fixture.nodes.get(root.collections[0].domain)!) {
    node.instance.block.run.origin = { type: 'scheduled', start_id: 'scheduled-start-a' };
  }
  const initial: TraceCache = { ...fixture.cache, page: { records: [root], next_cursor: 'earlier' } };
  const nativePage = fixture.reader.page, nativeValue = fixture.reader.value;
  const oldValue = vi.fn(nativeValue);
  fixture.reader.value = oldValue;
  const pending: { offset: number; resolve: () => void; reject: (error: Error) => void }[] = [];
  const pageReads: number[] = [];
  const reader: TraceFieldReader = {
    ...fixture.reader,
    page: async (...args) => {
      pageReads.push(args[2]);
      const page = await nativePage(...args);
      return new Promise<CheckpointPage>((resolve, reject) => { pending.push({ offset: args[2], resolve: () => resolve(page), reject }); });
    },
    value: vi.fn(async (...args: Parameters<TraceFieldReader['value']>) => {
      const value = await nativeValue(...args);
      return { ...value, bytes_base64: btoa(atob(value.bytes_base64).replaceAll('output-', 'latest-')) };
    }),
  };
  const loadDetail = vi.fn(), onSelect = vi.fn();
  let prepended: TraceCache | undefined;
  function History() {
    const [cache, setCache] = useState(initial);
    return <Trajectory cache={cache} onSelect={onSelect} onLoadDetail={loadDetail} loadEarlier={() => {
      prepended = { ...prependTrace(cache, { records: [traceRecord(2, { kind: 'user', request: null })], read_cut: fixture.cut, next_cursor: null }), fields: reader };
      setCache(prepended);
    }} />;
  }
  return {
    ...fixture, root, oldValue, reader, pending, pageReads, loadDetail, onSelect, History,
    get prepended() { return prepended; },
    async page(offset: number) {
      await waitFor(() => expect(pending[0]?.offset).toBe(offset));
      await act(async () => pending.shift()!.resolve());
    },
    async fail(offset: number) {
      await waitFor(() => expect(pending[0]?.offset).toBe(offset));
      await act(async () => pending.shift()!.reject(Error('request_capacity_exceeded')));
    },
  };
}

async function selectResult(container: HTMLElement) {
  fireEvent.click(screen.getByRole('button', { name: 'Expand workflow nodes' }));
  await waitFor(() => expect(container.querySelectorAll('[data-workflow-node]')).toHaveLength(73));
  fireEvent.click(container.querySelector('[data-workflow-node="node-0-60"]')!);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('each / body / 60 / visit'));
  fireEvent.click(screen.getByRole('tab', { name: 'Result' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('output-60'));
}

it('preserves a Scheduled visit and Result facet across cold Load Earlier with a new reader in the same epoch', async () => {
  const fixture = scheduledHistory(), ui = render(<fixture.History />);
  await selectResult(ui.container);
  const key = ui.container.querySelector('[data-workflow-node="node-0-60"]')!.getAttribute('data-display-key');
  const oldReads = fixture.oldValue.mock.calls.length;
  fireEvent.click(screen.getAllByRole('button', { name: 'Load earlier history' })[0]);
  expect(fixture.prepended?.epoch).toBe(fixture.cache.epoch);
  expect(fixture.prepended?.fields).toBe(fixture.reader);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(ui.container.querySelector('[data-display-type="RecordRow"][data-owner="trace:30"]')?.getAttribute('aria-selected')).toBe('false');
  expect(screen.getByRole('status').getAttribute('aria-busy')).toBe('true');
  expect(screen.getByRole('complementary', { name: 'Event details' }).textContent).not.toContain('output-60');
  expect(screen.queryByRole('tabpanel')).toBeNull();
  expect(screen.getByRole('button', { name: 'Close details' })).toBeTruthy();
  await fixture.page(0);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(screen.getByRole('status').getAttribute('aria-busy')).toBe('true');
  await fixture.page(32);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(screen.getByRole('status').getAttribute('aria-busy')).toBe('true');
  await fixture.page(64);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('latest-60'));
  const selected = ui.container.querySelector('[data-workflow-node][aria-selected="true"]');
  expect(selected?.getAttribute('data-workflow-node')).toBe('node-0-60');
  expect(selected?.getAttribute('data-display-key')).toBe(key);
  expect(screen.getByRole('tab', { name: 'Result' }).getAttribute('aria-selected')).toBe('true');
  expect(fixture.oldValue.mock.calls).toHaveLength(oldReads);
  expect(fixture.reader.value).toHaveBeenCalledWith(fixture.cut, expect.objectContaining({ domain: 'journal.output-node-0-60' }), 0, expect.any(Function));
  expect(fixture.loadDetail).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('each / body / 60 / visit'));
});

it('keeps a failed replacement in the selected Inspector and retries the same visit and Result using only its current reader', async () => {
  const fixture = scheduledHistory(), ui = render(<fixture.History />);
  await selectResult(ui.container);
  const key = ui.container.querySelector('[data-workflow-node="node-0-60"]')!.getAttribute('data-display-key');
  const oldReads = fixture.oldValue.mock.calls.length, selections = fixture.onSelect.mock.calls.length;
  fireEvent.click(screen.getAllByRole('button', { name: 'Load earlier history' })[0]);
  await fixture.page(0);
  await fixture.fail(32);
  const failed = screen.getByRole('complementary', { name: 'Event details' });
  expect(within(failed).getByRole('alert').textContent).toContain('request_capacity_exceeded');
  expect(screen.getAllByRole('alert')).toHaveLength(1);
  expect(within(failed).getByRole('button', { name: 'Close details' })).toBeTruthy();
  expect(failed.textContent).not.toContain('output-60');
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(ui.container.querySelector('[data-display-type="RecordRow"][data-owner="trace:30"]')?.getAttribute('aria-selected')).toBe('false');
  expect(fixture.pageReads).toEqual([0, 32]);
  fireEvent.click(within(failed).getByRole('button', { name: 'Retry loading details' }));
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.getByRole('status').getAttribute('aria-busy')).toBe('true');
  await fixture.page(0);
  await fixture.page(32);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(screen.getByRole('complementary', { name: 'Event details' }).textContent).not.toContain('output-60');
  await fixture.page(64);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('latest-60'));
  expect(ui.container.querySelector('[data-workflow-node="node-0-60"]')?.getAttribute('aria-selected')).toBe('true');
  expect(ui.container.querySelector('[data-workflow-node="node-0-60"]')?.getAttribute('data-display-key')).toBe(key);
  expect(screen.getByRole('tab', { name: 'Result' }).getAttribute('aria-selected')).toBe('true');
  expect(fixture.pageReads).toEqual([0, 32, 0, 32, 64]);
  expect(fixture.oldValue.mock.calls).toHaveLength(oldReads);
  expect(fixture.onSelect.mock.calls).toHaveLength(selections);
  expect(fixture.loadDetail).not.toHaveBeenCalled();
});

it.each([
  { action: 'collapse', retry: false }, { action: 'close', retry: false },
  { action: 'collapse', retry: true }, { action: 'close', retry: true },
] as const)('respects $action after a replacement error (retry in flight: $retry)', async ({ action, retry }) => {
  const fixture = scheduledHistory(), ui = render(<fixture.History />);
  await selectResult(ui.container);
  const oldReads = fixture.oldValue.mock.calls.length;
  fireEvent.click(screen.getAllByRole('button', { name: 'Load earlier history' })[0]);
  await fixture.page(0);
  await fixture.fail(32);
  if (retry) {
    fireEvent.click(screen.getByRole('button', { name: 'Retry loading details' }));
    await waitFor(() => expect(fixture.pending[0]?.offset).toBe(0));
  }
  fireEvent.click(screen.getByRole('button', { name: action === 'collapse' ? 'Collapse workflow nodes' : 'Close details' }));
  expect(screen.queryByRole('complementary', { name: 'Event details' })).toBeNull();
  if (action === 'collapse') {
    if (retry) await fixture.page(0);
    expect(fixture.pending).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: 'Expand workflow nodes' }));
  } else if (!retry) {
    // The failed expansion remains recoverable after closing its Inspector.
    fireEvent.click(screen.getByRole('button', { name: 'Retry loading details' }));
  }
  await fixture.page(0);
  await fixture.page(32);
  await fixture.page(64);
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73));
  expect(ui.container.querySelector('[data-workflow-node][aria-selected="true"]')).toBeNull();
  expect(screen.queryByRole('complementary', { name: 'Event details' })).toBeNull();
  expect(fixture.oldValue.mock.calls).toHaveLength(oldReads);
});

it.each(['collapse', 'close'] as const)('respects an explicit %s while the selected visit is unresolved by its replacement reader', async action => {
  const fixture = scheduledHistory(), ui = render(<fixture.History />);
  await selectResult(ui.container);
  fireEvent.click(screen.getAllByRole('button', { name: 'Load earlier history' })[0]);
  await waitFor(() => expect(fixture.pending).toHaveLength(1));
  fireEvent.click(screen.getByRole('button', { name: action === 'collapse' ? 'Collapse workflow nodes' : 'Close details' }));
  expect(screen.queryByRole('complementary', { name: 'Event details' })).toBeNull();
  if (action === 'collapse') {
    // A page from the collapsed expansion cannot restore a latent selection.
    await fixture.page(0);
    expect(fixture.pending).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: 'Expand workflow nodes' }));
  }
  await fixture.page(0);
  await fixture.page(32);
  await fixture.page(64);
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73));
  expect(ui.container.querySelector('[data-workflow-node][aria-selected="true"]')).toBeNull();
  expect(screen.queryByRole('complementary', { name: 'Event details' })).toBeNull();
});

it('resolves a Workflow selection only by its complete native visit identity, never a run or sibling fallback', () => {
  const fixture = scheduledHistory(), node = fixture.nodes.get(fixture.root.collections[0].domain)![60];
  const item: Extract<InspectableDisplayItem, { type: 'WorkflowNode' }> = {
    type: 'WorkflowNode', record: fixture.root, owner_record_id: fixture.root.id, parent_record_id: fixture.root.id,
    display_key: JSON.stringify(['workflow-node', fixture.root.id, node.id]), label: node.instance.node, preview: '', facet: 'overview',
    workflowNode: node, workflowSource: { root: fixture.root, nodes: [node], cut: fixture.cut, reader: fixture.reader, loading: false },
  };
  const selection = { ...selectionOf(item), facet: 'output' as const };
  const root: InspectableDisplayItem = { type: 'RecordRow', record: fixture.root, owner_record_id: fixture.root.id,
    display_key: 'scheduled-root', label: 'run', preview: '', facet: 'overview' };
  const sibling = { ...item, display_key: 'sibling', workflowNode: fixture.nodes.get(fixture.root.collections[0].domain)![0] };
  expect(preferredItem([root, sibling], fixture.root.id, selection)).toBeUndefined();
  expect(preferredItem([root, sibling, item], fixture.root.id, selection)).toBe(item);
  const mutate = [
    (value: WorkflowTraceNode) => { value.id = 'other-node-id'; },
    (value: WorkflowTraceNode) => { value.instance.block.run.conversation_id = 'other-conversation'; },
    (value: WorkflowTraceNode) => { value.instance.block.run.origin = { type: 'scheduled', start_id: 'other-start' }; },
    (value: WorkflowTraceNode) => { value.instance.block.run.origin = { type: 'tool', attempt_id: 'attempt-a' }; },
    (value: WorkflowTraceNode) => { value.instance.block.run.invocation = '99'; },
    (value: WorkflowTraceNode) => { value.instance.block.definition.workflow_id = 'other-workflow'; },
    (value: WorkflowTraceNode) => { value.instance.block.definition.blocks = ['other', 'body']; },
    (value: WorkflowTraceNode) => { value.instance.block.invocations = [61]; },
    (value: WorkflowTraceNode) => { value.instance.node = 'other-node'; },
    (value: WorkflowTraceNode) => { value.instance.visit += 1; },
  ];
  for (const change of mutate) {
    const changed = structuredClone(node);
    change(changed);
    expect(preferredItem([root, sibling, { ...item, workflowNode: changed }], fixture.root.id, selection)).toBeUndefined();
  }
});
