import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CheckpointPage, CheckpointReadCut } from '../../protocol/app-server/v73';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { refreshTrace } from '../src/client/trace';
import { workflowTraceFixture } from './workflow-trace-fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

function controlledRefresh() {
  const fixture = workflowTraceFixture(73);
  const fresh = { ...fixture.cut, revision: '501', through: '501' };
  const nativePage = fixture.reader.page, nativeValue = fixture.reader.value;
  const pending: { offset: number; page: CheckpointPage; settled: boolean; resolve: (page: CheckpointPage) => void; reject: (error: Error) => void }[] = [];
  const valueReads: { cut: CheckpointReadCut; domain: string }[] = [];
  fixture.reader.page = async (cut, domain, offset, current) => {
    const page = await nativePage(fixture.cut, domain, offset, current);
    if (cut.revision === fixture.cut.revision) return page;
    return new Promise<CheckpointPage>((resolve, reject) => {
      pending.push({ offset, page: { ...page, cursor: cut.revision, read_cut: cut }, settled: false, resolve, reject });
    });
  };
  fixture.reader.value = async (cut, field, offset, current) => {
    valueReads.push({ cut, domain: field.domain });
    const chunk = await nativeValue(fixture.cut, field, offset, current);
    if (cut.revision === fixture.cut.revision) return chunk;
    // Keep selector byte lengths unchanged, while exposing which cut supplied
    // the Inspector's actual bodies rather than relying on preview text.
    const body = atob(chunk.bytes_base64).replaceAll('output-', 'latest-').replaceAll('input-', 'fresh-');
    return { ...chunk, cut, bytes_base64: btoa(body) };
  };
  return {
    ...fixture, fresh, pending, valueReads,
    refreshed: refreshTrace(fixture.cache, { records: fixture.cache.page.records, read_cut: fresh, next_cursor: null }),
    async page(offset: number) {
      await waitFor(() => expect(pending.some(read => read.offset === offset && !read.settled)).toBe(true));
      await act(async () => { const read = pending.find(read => read.offset === offset && !read.settled)!; read.settled = true; read.resolve(read.page); });
    },
  };
}

function trajectory(cache: ReturnType<typeof workflowTraceFixture>['cache']) {
  return <Trajectory cache={cache} loadEarlier={() => {}} onSelect={() => {}} onLoadDetail={() => {}} />;
}

function openWorkflow(container: HTMLElement) {
  fireEvent.click(container.querySelector('[data-trace-id="trace:10"] button[aria-expanded]')!);
}

function selectedVisit(container: HTMLElement) {
  const selected = container.querySelector('[data-workflow-node][aria-selected="true"]');
  expect(selected?.getAttribute('data-workflow-node')).toBe('node-0-60');
  expect(selected?.getAttribute('data-display-key')).toBe(JSON.stringify(['workflow-node', 'trace:30', 'node-0-60']));
}

async function selectLaterVisit(container: HTMLElement) {
  await waitFor(() => expect(container.querySelectorAll('[data-workflow-node]')).toHaveLength(73));
  fireEvent.click(container.querySelector('[data-workflow-node="node-0-60"]')!);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('each / body / 60 / visit'));
  selectedVisit(container);
  fireEvent.click(screen.getByRole('tab', { name: 'Result' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('output-60'));
}

it('does not expose a partial workflow before its first complete snapshot has been admitted', async () => {
  const fixture = controlledRefresh();
  const ui = render(trajectory(fixture.refreshed));
  openWorkflow(ui.container);
  await fixture.page(0);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  await fixture.page(32);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  await fixture.page(64);
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73));
});

it('refreshes a complete paged workflow atomically without changing the selected visit or rebinding old bodies', async () => {
  const fixture = controlledRefresh();
  const ui = render(trajectory(fixture.cache));
  openWorkflow(ui.container);
  await selectLaterVisit(ui.container);
  ui.rerender(trajectory(fixture.refreshed));
  await fixture.page(0);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73);
  selectedVisit(ui.container);
  expect(screen.getByRole('tabpanel').textContent).toContain('output-60');
  expect(fixture.valueReads.some(read => read.cut.revision === fixture.fresh.revision)).toBe(false);
  // A new facet read during the refresh must still use the complete old
  // snapshot's cut, even though the outer Trace advertises the new one.
  fireEvent.click(screen.getByRole('tab', { name: 'Payload' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('input-60'));
  expect(fixture.valueReads.at(-1)).toEqual({ cut: fixture.cut, domain: 'journal.input-node-0-60' });
  await fixture.page(32);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73);
  selectedVisit(ui.container);
  expect(screen.getByRole('tabpanel').textContent).toContain('input-60');
  await fixture.page(64);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('fresh-60'));
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73);
  selectedVisit(ui.container);
  fireEvent.click(screen.getByRole('tab', { name: 'Result' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('latest-60'));
  expect(fixture.valueReads.at(-1)).toEqual({ cut: fixture.fresh, domain: 'journal.output-node-0-60' });
  fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('each / body / 60 / visit'));
});

it('retains the selected complete snapshot and original body source when a replacement page fails validation', async () => {
  const fixture = controlledRefresh();
  const ui = render(trajectory(fixture.cache));
  openWorkflow(ui.container);
  await selectLaterVisit(ui.container);
  ui.rerender(trajectory(fixture.refreshed));
  await fixture.page(0);
  await waitFor(() => expect(fixture.pending.some(read => read.offset === 32)).toBe(true));
  await act(async () => {
    const read = fixture.pending.find(read => read.offset === 32)!;
    read.resolve({ ...read.page, read_cut: fixture.cut });
  });
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Original Workflow node page changed'));
  expect(fixture.pending.map(read => read.offset)).toEqual([0, 32]);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73);
  selectedVisit(ui.container);
  fireEvent.click(screen.getByRole('tab', { name: 'Payload' }));
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('input-60'));
  expect(fixture.valueReads.at(-1)).toEqual({ cut: fixture.cut, domain: 'journal.input-node-0-60' });
  expect(fixture.valueReads.some(read => read.cut.revision === fixture.fresh.revision)).toBe(false);
});

it('retries a failed same-reader refresh from the top error without replacing its selected complete snapshot early', async () => {
  const fixture = controlledRefresh(), ui = render(trajectory(fixture.cache));
  openWorkflow(ui.container);
  await selectLaterVisit(ui.container);
  ui.rerender(trajectory(fixture.refreshed));
  await fixture.page(0);
  await waitFor(() => expect(fixture.pending.some(read => read.offset === 32)).toBe(true));
  await act(async () => {
    const read = fixture.pending.find(read => read.offset === 32)!;
    read.settled = true; read.reject(Error('request_capacity_exceeded'));
  });
  expect(screen.getByRole('alert').textContent).toContain('request_capacity_exceeded');
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73);
  selectedVisit(ui.container);
  expect(screen.getByRole('tabpanel').textContent).toContain('output-60');
  fireEvent.click(screen.getByRole('button', { name: 'Retry loading details' }));
  await fixture.page(0);
  await fixture.page(32);
  expect(screen.queryByRole('alert')).toBeNull();
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(73);
  selectedVisit(ui.container);
  expect(screen.getByRole('tabpanel').textContent).toContain('output-60');
  expect(fixture.valueReads.some(read => read.cut.revision === fixture.fresh.revision)).toBe(false);
  await fixture.page(64);
  await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('latest-60'));
  selectedVisit(ui.container);
  expect(screen.getByRole('tab', { name: 'Result' }).getAttribute('aria-selected')).toBe('true');
  expect(fixture.pending.map(read => read.offset)).toEqual([0, 32, 0, 32, 64]);
});

it('does not publish a retired workflow page or expose its complete snapshot to a replacement Trace owner', async () => {
  const fixture = controlledRefresh();
  const ui = render(trajectory(fixture.cache));
  openWorkflow(ui.container);
  await selectLaterVisit(ui.container);
  ui.rerender(trajectory(fixture.refreshed));
  await fixture.page(0);
  await waitFor(() => expect(fixture.pending.some(read => read.offset === 32)).toBe(true));
  const replacement = workflowTraceFixture(2);
  const replacementPage = replacement.reader.page;
  let finish!: () => void;
  replacement.reader.page = async (...args) => {
    const page = await replacementPage(...args);
    return new Promise(resolve => { finish = () => resolve(page); });
  };
  ui.rerender(trajectory({ ...replacement.cache, epoch: fixture.cache.epoch + 1 }));
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(screen.queryByRole('tabpanel')).toBeNull();
  openWorkflow(ui.container);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await fixture.page(32);
  expect(fixture.pending.map(read => read.offset)).toEqual([0, 32]);
  expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(0);
  expect(screen.queryByRole('tabpanel')).toBeNull();
  await act(async () => finish());
  await waitFor(() => expect(ui.container.querySelectorAll('[data-workflow-node]')).toHaveLength(2));
  expect(ui.container.querySelector('[data-workflow-node="node-0-60"]')).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
});
