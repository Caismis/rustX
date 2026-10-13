// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ProjectionHistory } from '../src/app/agent/ProjectionHistory';
import { Server, projectionHead } from './fixture';
import type { ProjectionDetail } from '../../protocol/app-server/v47';
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
