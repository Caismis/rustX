import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { projectTrajectory, trajectoryItems } from '../src/app/trajectory/layout';
import { workflowCalls } from '../src/app/trajectory/workflow';
import { useWorkflowNodeContent } from '../src/app/trajectory/useWorkflowNodeContent';
import type { InspectableDisplayItem } from '../src/app/trajectory/layout';
import { refreshTrace } from '../src/client/trace';
import { useWorkflowNodes } from '../src/app/trajectory/useWorkflowNodes';
import { translator } from '../src/locale/translation';
import { workflowTraceFixture } from './workflow-trace-fixture';
import type { CheckpointPage } from '../../protocol/app-server/v69';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
it('pairs repeated workflow names only by native call and Step and removes their duplicate lifecycle bars', () => {
  const { cache, roots, calls } = workflowTraceFixture(3),
    tx = translator('en');
  const matched = workflowCalls(cache.page.records);
  expect(matched.get(calls[0].id)?.id).toBe(roots[0].id);
  expect(matched.get(calls[1].id)?.id).toBe(roots[1].id);
  expect(matched.has('trace:20')).toBe(false);
  const items = trajectoryItems(tx, projectTrajectory(tx, cache.page.records));
  expect(items.filter((item) => 'record' in item && item.record.kind === 'workflow')).toHaveLength(0);
  const wrong = { ...roots[0], location: { attempt_id: 'other-attempt', step_id: '1' } };
  expect(workflowCalls([calls[0], wrong]).size).toBe(0);
});

it('does not read closed workflows, then reads every native node page once in admission order', async () => {
  const { cache, roots, reads, nodes } = workflowTraceFixture(73);
  const ui = renderHook(({ opened }) => useWorkflowNodes(cache, roots, opened), {
    initialProps: { opened: new Set<string>() },
  });
  expect(reads).toHaveLength(0);
  ui.rerender({ opened: new Set([roots[0].id]) });
  await waitFor(() => expect(ui.result.current.get(roots[0].id)?.loading).toBe(false));
  expect(reads.map((read) => read.offset)).toEqual([0, 32, 64]);
  expect(ui.result.current.get(roots[0].id)?.nodes.map((node) => node.id)).toEqual(
    nodes.get(roots[0].collections[0].domain)!.map((node) => node.id),
  );
});

it('a late node page from a retired Trace epoch cannot populate the replacement', async () => {
  const fixture = workflowTraceFixture(2);
  let finish!: (value: CheckpointPage) => void;
  const real = fixture.reader.page,
    page = await real(fixture.cut, fixture.roots[0].collections[0].domain, 0, () => true);
  fixture.reader.page = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  const opened = new Set([fixture.roots[0].id]);
  const ui = renderHook(({ cache }) => useWorkflowNodes(cache, fixture.roots, opened), {
    initialProps: { cache: fixture.cache },
  });
  const old = finish;
  ui.rerender({ cache: { ...fixture.cache, epoch: fixture.cache.epoch + 1 } });
  await act(async () => old(page));
  expect(ui.result.current.get(fixture.roots[0].id)?.nodes).toHaveLength(0);
});

it('expands native node rows with their own input/output Inspector and keeps unsafe integers exact', async () => {
  const fixture = workflowTraceFixture(4),
    load = vi.fn();
  const ui = render(
    <Trajectory cache={fixture.cache} loadEarlier={() => {}} onSelect={() => {}} onLoadDetail={load} />,
  );
  const call = ui.container.querySelector('[data-trace-id="trace:10"]')!;
  expect(call.getAttribute('data-kind')).toBe('workflow');
  expect(ui.container.querySelector('[data-trace-id="trace:20"]')?.getAttribute('data-kind')).toBe('tool');
  fireEvent.click(call.querySelector('button[aria-expanded]')!);
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(4));
  fireEvent.click(ui.container.querySelector('[data-workflow-node="node-0-0"]')!);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('18446744073709551615'));
  expect(screen.getByRole('tab', { name: 'Payload' })).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Result' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('output-0'));
  expect(load).not.toHaveBeenCalled();
  fireEvent.click(call.querySelector('button[aria-expanded]')!);
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0));
});

it('distinguishes incomplete historical visits from positively evidenced live waits', async () => {
  const fixture = workflowTraceFixture(2);
  const nodes = fixture.nodes.get(fixture.roots[0].collections[0].domain)!;
  nodes[0].state = { type: 'incomplete' };
  nodes[1].state = { type: 'waiting', reason: 'review' };
  for (const node of nodes) { node.output = null; node.output_preview = null; }
  const ui = render(<Trajectory cache={fixture.cache} loadEarlier={() => {}} onSelect={() => {}} onLoadDetail={() => {}} />);
  fireEvent.click(ui.container.querySelector('[data-trace-id="trace:10"] button[aria-expanded]')!);
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(2));
  const historical = ui.container.querySelector('[data-workflow-node="node-0-0"]')!;
  const live = ui.container.querySelector('[data-workflow-node="node-0-1"]')!;
  expect(historical.getAttribute('data-state')).toBe('incomplete');
  expect(historical.hasAttribute('data-running')).toBe(false);
  expect(historical.textContent).toContain('incomplete');
  expect(live.getAttribute('data-state')).toBe('waiting');
  expect(live.getAttribute('data-running')).toBe('true');
  fireEvent.click(historical);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('incomplete'));
});

it('reads an unchanged workflow at its freshly advertised native cut after the old cut expires', async () => {
  const fixture = workflowTraceFixture(2),
    fresh = { ...fixture.cut, revision: '501', through: '501' };
  const native = fixture.reader.page;
  fixture.reader.page = async (cut, domain, offset, current) => {
    if (cut.revision !== fresh.revision) throw Error('read_cut_expired');
    const page = await native(fixture.cut, domain, offset, current);
    return { ...page, cursor: fresh.revision, read_cut: fresh };
  };
  const refreshed = refreshTrace(fixture.cache, {
    records: fixture.cache.page.records,
    read_cut: fresh,
    next_cursor: null,
  });
  const ui = renderHook(() => useWorkflowNodes(refreshed, fixture.roots, new Set([fixture.roots[0].id])));
  await waitFor(() => expect(ui.result.current.get(fixture.roots[0].id)?.loading).toBe(false));
  expect(ui.result.current.get(fixture.roots[0].id)?.error).toBeUndefined();
  expect(ui.result.current.get(fixture.roots[0].id)?.cut).toEqual(fresh);
  expect(ui.result.current.get(fixture.roots[0].id)?.nodes).toHaveLength(2);
  // The fresh row cut does not replace the independently retained Tool body.
  expect(refreshed.readCuts?.[fixture.calls[0].id]).toEqual(fixture.cut);
});

it('repages an open workflow when its cut changes but its collection revision does not', async () => {
  const fixture = workflowTraceFixture(2),
    opened = new Set([fixture.roots[0].id]);
  const native = fixture.reader.page,
    seen: string[] = [];
  fixture.reader.page = async (cut, domain, offset, current) => {
    seen.push(cut.revision);
    const page = await native(fixture.cut, domain, offset, current);
    return { ...page, cursor: cut.revision, read_cut: cut };
  };
  const ui = renderHook(({ cache }) => useWorkflowNodes(cache, fixture.roots, opened), {
    initialProps: { cache: fixture.cache },
  });
  await waitFor(() => expect(ui.result.current.get(fixture.roots[0].id)?.loading).toBe(false));
  const fresh = { ...fixture.cut, revision: '501', through: '501' };
  ui.rerender({
    cache: refreshTrace(fixture.cache, { records: fixture.cache.page.records, read_cut: fresh, next_cursor: null }),
  });
  await waitFor(() => expect(seen).toEqual(['500', '501']));
  await waitFor(() => expect(ui.result.current.get(fixture.roots[0].id)?.cut).toEqual(fresh));
});

it('replaces an expired node-body read when the native cut advances without changing its selectors', async () => {
  const fixture = workflowTraceFixture(1),
    root = fixture.roots[0],
    node = fixture.nodes.get(root.collections[0].domain)![0];
  const native = fixture.reader.value;
  let rejectOld!: (error: Error) => void;
  fixture.reader.value = async (cut, field, offset, current) => {
    if (cut.revision === '500')
      return new Promise((_, reject) => {
        rejectOld = reject;
      });
    const chunk = await native(fixture.cut, field, offset, current);
    return { ...chunk, cut };
  };
  const item = (cut: typeof fixture.cut): InspectableDisplayItem => ({
    type: 'WorkflowNode',
    record: root,
    owner_record_id: root.id,
    parent_record_id: fixture.calls[0].id,
    display_key: 'node',
    label: node.instance.node,
    preview: '',
    facet: 'overview',
    workflowNode: node,
    workflowSource: { root, nodes: [node], cut, reader: fixture.reader, loading: false },
  });
  const ui = renderHook(({ cut }) => useWorkflowNodeContent(item(cut), 'input'), {
    initialProps: { cut: fixture.cut },
  });
  await waitFor(() => expect(rejectOld).toBeTypeOf('function'));
  ui.rerender({ cut: { ...fixture.cut, revision: '501', through: '501' } });
  await waitFor(() => expect(ui.result.current?.values?.input).toBeDefined());
  await act(async () => rejectOld(Error('read_cut_expired')));
  expect(ui.result.current?.error).toBeUndefined();
  expect(ui.result.current?.values?.input).toBeDefined();
});

it('an admitted workflow with no started nodes has a valid empty expansion', async () => {
  const fixture = workflowTraceFixture(0),
    ui = renderHook(() => useWorkflowNodes(fixture.cache, fixture.roots, new Set([fixture.roots[0].id])));
  await waitFor(() => expect(ui.result.current.get(fixture.roots[0].id)?.loading).toBe(false));
  expect(ui.result.current.get(fixture.roots[0].id)?.error).toBeUndefined();
  expect(ui.result.current.get(fixture.roots[0].id)?.nodes).toEqual([]);
});

it('an unrelated fresh Trace page cannot rebind a retained workflow to its cut', () => {
  const fixture = workflowTraceFixture(2),
    fresh = { ...fixture.cut, revision: '501', through: '501' };
  const cache = refreshTrace(fixture.cache, { records: [fixture.calls[0]], read_cut: fresh, next_cursor: null });
  expect(cache.readCuts?.[fixture.roots[0].id]).toEqual(fixture.cut);
});
