// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ProjectionHistory } from '../src/app/agent/ProjectionHistory';
import { Server, projectionHead } from './fixture';
import type { ProjectionDetail } from '../../protocol/app-server/v48';
afterEach(cleanup);
it('reads historical activations and typed occurrences on demand without manufacturing execution rows', async () => {
  const server = new Server(); await server.attached('A');
  const first = { ...projectionHead('Local summary'), checkpoint_id: 'P1' };
  const second = { ...projectionHead('Inherited summary', true), checkpoint_id: 'P2', activation_id: '2' };
  server.handlers.set('session/projections', () => ({ type: 'projections', page: { records: [first, second], next_activation: null } }));
  const detail: ProjectionDetail = { checkpoint: second, next_item: null, items: [{ occurrence: { checkpoint_id: 'P2', item_index: 3 }, content: { request_occurrence: null, role: 'user', message_id: null, source: 'Runtime', provenance: { type: 'derived', sources: [], checkpoint_sources: [], transformations: ['summary'], truncated: false }, blocks: [{ type: 'text', text: { text: 'Typed projected detail', truncated: false } }], truncated: false } }] };
  server.handlers.set('session/projectionDetail', () => ({ type: 'projection_detail', detail }));
  const view = render(<ProjectionHistory client={server.client} sessionId="A"/>);
  expect(server.requests.some(item => item.request.method === 'session/projections')).toBe(false);
  await act(async () => { const disclosure = view.container.querySelector('details')!; disclosure.open = true; fireEvent(disclosure, new Event('toggle')); });
  expect(view.container.querySelector('[data-projection-origin="local"]')).not.toBeNull();
  expect(view.container.querySelector('[data-projection-origin="inherited"]')).not.toBeNull();
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[1]!); });
  expect(screen.getByText('Typed projected detail')).not.toBeNull();
  expect(view.container.querySelector('[data-projection-detail="P2"]')).not.toBeNull();
  expect(view.container.querySelector('[data-turn-owner]')).toBeNull();
  expect(server.requests.some(item => item.request.method === 'turn/start')).toBe(false);
  await server.client.disconnect();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function head(id: number) { return { ...projectionHead(`Summary ${id}`), checkpoint_id: `P${id}`, activation_id: String(id) }; }
function detailPage(id: number, indexes: number[], next: number | null): ProjectionDetail {
  return { checkpoint: head(id), next_item: next, items: indexes.map(item_index => ({
    occurrence: { checkpoint_id: `P${id}`, item_index },
    content: { request_occurrence: null, role: 'user', message_id: null, source: 'Runtime',
      provenance: { type: 'derived', sources: [], checkpoint_sources: [], transformations: ['summary'], truncated: false },
      blocks: [{ type: 'text', text: { text: `P${id} item ${item_index}`, truncated: false } }], truncated: false },
  })) };
}
async function openHistory(container: HTMLElement) {
  await act(async () => { const node = container.querySelector('details')!; node.open = true; fireEvent(node, new Event('toggle')); });
}
it('accumulates overlapping activation and detail pages with stable records and exhausted cursors', async () => {
  const server = new Server();
  const pages = vi.spyOn(server.client, 'projections')
    .mockResolvedValueOnce({ records: [head(1)], next_activation: '1' })
    .mockResolvedValueOnce({ records: [{ ...head(1), summary_preview: { text: 'changed duplicate', truncated: false } }, head(2)], next_activation: '2' })
    .mockResolvedValueOnce({ records: [head(2), head(3)], next_activation: null });
  const details = vi.spyOn(server.client, 'projectionDetail')
    .mockResolvedValueOnce(detailPage(1, [0, 1], 2))
    .mockResolvedValueOnce(detailPage(1, [1, 2], null));
  const view = render(<ProjectionHistory client={server.client} sessionId="A"/>);
  await openHistory(view.container);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more' })); });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more' })); });
  expect(screen.getAllByRole('button', { name: 'Inspect projected messages' })).toHaveLength(3);
  expect(screen.queryByText('changed duplicate')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull();
  expect(pages.mock.calls.map(call => call[1])).toEqual(['0', '1', '2']);
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[0]!); });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more' })); });
  expect([...view.container.querySelectorAll('[data-projection-detail] article')].map(node => node.textContent)).toEqual([
    expect.stringContaining('P1 item 0'), expect.stringContaining('P1 item 1'), expect.stringContaining('P1 item 2'),
  ]);
  expect(details.mock.calls.map(call => call[2])).toEqual([0, 2]);
  expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull();
});
it('a newly selected checkpoint supersedes an in-flight detail without mixing its pages or errors', async () => {
  const server = new Server();
  vi.spyOn(server.client, 'projections').mockResolvedValue({ records: [head(1), head(2)], next_activation: null });
  const old = deferred<ProjectionDetail>();
  vi.spyOn(server.client, 'projectionDetail').mockReturnValueOnce(old.promise).mockResolvedValueOnce(detailPage(2, [0], null));
  const view = render(<ProjectionHistory client={server.client} sessionId="A"/>);
  await openHistory(view.container);
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[0]!); });
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[1]!); });
  await act(async () => { old.resolve(detailPage(1, [0], null)); });
  expect(view.container.querySelector('[data-projection-detail="P2"]')).not.toBeNull();
  expect(screen.queryByText('P1 item 0')).toBeNull();
});
it('session replacement rejects pending activation and detail responses even when returning to the original session', async () => {
  const server = new Server();
  const old = deferred<Awaited<ReturnType<typeof server.client.projections>>>();
  const oldDetail = deferred<ProjectionDetail>();
  vi.spyOn(server.client, 'projections').mockResolvedValueOnce({ records: [head(1)], next_activation: '1' })
    .mockReturnValueOnce(old.promise).mockResolvedValueOnce({ records: [head(2)], next_activation: null })
    .mockResolvedValueOnce({ records: [head(3)], next_activation: null });
  vi.spyOn(server.client, 'projectionDetail').mockReturnValueOnce(oldDetail.promise);
  const view = render(<ProjectionHistory client={server.client} sessionId="A"/>);
  await openHistory(view.container);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Inspect projected messages' })); });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more' })); });
  view.rerender(<ProjectionHistory client={server.client} sessionId="B"/>);
  await openHistory(view.container);
  view.rerender(<ProjectionHistory client={server.client} sessionId="A"/>);
  await openHistory(view.container);
  await act(async () => { old.resolve({ records: [head(1)], next_activation: null }); oldDetail.reject(new Error('obsolete failure')); });
  expect(screen.getAllByRole('button', { name: 'Inspect projected messages' })).toHaveLength(1);
  expect(view.container.querySelector('[data-projection-detail]')).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Inspect projected messages' })); });
  expect(server.client.projectionDetail).toHaveBeenLastCalledWith('A', 'P3', 0);
});
it('later-page failures retain previous history and detail and allow retry without concurrent duplicates', async () => {
  const server = new Server();
  const later = deferred<Awaited<ReturnType<typeof server.client.projections>>>();
  vi.spyOn(server.client, 'projections').mockResolvedValueOnce({ records: [head(1)], next_activation: '1' })
    .mockReturnValueOnce(later.promise).mockResolvedValueOnce({ records: [head(2)], next_activation: null });
  vi.spyOn(server.client, 'projectionDetail').mockResolvedValueOnce(detailPage(1, [0], 1))
    .mockRejectedValueOnce(new Error('detail unavailable')).mockResolvedValueOnce(detailPage(1, [1], null));
  const view = render(<ProjectionHistory client={server.client} sessionId="A"/>);
  await openHistory(view.container);
  const more = screen.getByRole('button', { name: 'Load more' });
  await act(async () => { fireEvent.click(more); fireEvent.click(more); });
  expect(server.client.projections).toHaveBeenCalledTimes(2);
  await act(async () => { later.reject(new Error('page unavailable')); });
  expect(screen.getByRole('button', { name: 'Inspect projected messages' })).toBeTruthy();
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more' })); });
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[0]!); });
  const detail = view.container.querySelector('[data-projection-detail]')!;
  await act(async () => { fireEvent.click(detail.querySelector('button')!); });
  expect(screen.getByText('P1 item 0')).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toContain('detail unavailable');
  await act(async () => { fireEvent.click(detail.querySelector('button')!); });
  expect(screen.getByText('P1 item 0')).toBeTruthy(); expect(screen.getByText('P1 item 1')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull();
});
it('switching selection rejects a pending later item page and keeps the new checkpoint occurrence order', async () => {
  const server = new Server();
  vi.spyOn(server.client, 'projections').mockResolvedValue({ records: [head(1), head(2)], next_activation: null });
  const oldPage = deferred<ProjectionDetail>();
  vi.spyOn(server.client, 'projectionDetail').mockResolvedValueOnce(detailPage(1, [0], 1))
    .mockReturnValueOnce(oldPage.promise).mockResolvedValueOnce(detailPage(2, [0, 1], null));
  const view = render(<ProjectionHistory client={server.client} sessionId="A"/>);
  await openHistory(view.container);
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[0]!); });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more' })); });
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'Inspect projected messages' })[1]!); });
  await act(async () => { oldPage.resolve(detailPage(1, [1, 2], null)); });
  expect([...view.container.querySelectorAll('[data-projection-detail] article')].map(node => node.textContent)).toEqual([
    expect.stringContaining('P2 item 0'), expect.stringContaining('P2 item 1'),
  ]);
  expect(screen.queryByText('P1 item 0')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull();
});
