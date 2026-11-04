import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { projectTrajectory, trajectoryItems } from '../src/app/trajectory/layout';
import { workflowCalls } from '../src/app/trajectory/workflow';
import { useWorkflowNodes } from '../src/app/trajectory/useWorkflowNodes';
import { translator } from '../src/locale/translation';
import { workflowTraceFixture } from './workflow-trace-fixture';
import type { CheckpointPage } from '../../protocol/app-server/v64';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
it('pairs repeated workflow names only by native call and Step and removes their duplicate lifecycle bars', () => {
  const { cache, roots, calls } = workflowTraceFixture(3), tx = translator('en');
  const matched = workflowCalls(cache.page.records);
  expect(matched.get(calls[0].id)?.id).toBe(roots[0].id);
  expect(matched.get(calls[1].id)?.id).toBe(roots[1].id);
  expect(matched.has('trace:20')).toBe(false);
  const items = trajectoryItems(tx, projectTrajectory(tx, cache.page.records));
  expect(items.filter(item => 'record' in item && item.record.kind === 'workflow')).toHaveLength(0);
  const wrong = { ...roots[0], location: { attempt_id: 'other-attempt', step_id: '1' } };
  expect(workflowCalls([calls[0], wrong]).size).toBe(0);
});

it('does not read closed workflows, then reads every native node page once in admission order', async () => {
  const { cache, roots, reads, nodes } = workflowTraceFixture(73);
  const ui = renderHook(({ opened }) => useWorkflowNodes(cache, roots, opened), { initialProps: { opened: new Set<string>() } });
  expect(reads).toHaveLength(0);
  ui.rerender({ opened: new Set([roots[0].id]) });
  await waitFor(() => expect(ui.result.current.get(roots[0].id)?.loading).toBe(false));
  expect(reads.map(read => read.offset)).toEqual([0, 32, 64]);
  expect(ui.result.current.get(roots[0].id)?.nodes.map(node => node.id)).toEqual(nodes.get(roots[0].collections[0].domain)!.map(node => node.id));
});

it('a late node page from a retired Trace epoch cannot populate the replacement', async () => {
  const fixture = workflowTraceFixture(2); let finish!: (value: CheckpointPage) => void;
  const real = fixture.reader.page, page = await real(fixture.cut, fixture.roots[0].collections[0].domain, 0, () => true);
  fixture.reader.page = () => new Promise(resolve => { finish = resolve; });
  const opened = new Set([fixture.roots[0].id]);
  const ui = renderHook(({ cache }) => useWorkflowNodes(cache, fixture.roots, opened), { initialProps: { cache: fixture.cache } });
  const old = finish;
  ui.rerender({ cache: { ...fixture.cache, epoch: fixture.cache.epoch + 1 } });
  await act(async () => old(page));
  expect(ui.result.current.get(fixture.roots[0].id)?.nodes).toHaveLength(0);
});

it('expands native node rows with their own input/output Inspector and keeps unsafe integers exact', async () => {
  const fixture = workflowTraceFixture(4), load = vi.fn();
  const ui = render(<Trajectory cache={fixture.cache} loadEarlier={() => {}} onSelect={() => {}} onLoadDetail={load}/>);
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
