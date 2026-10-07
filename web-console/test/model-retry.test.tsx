import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { modelRetryNotices } from '../src/bindings/model-retry';
import { ModelRetries } from '../src/app/agent/ModelRetry';
import { Server, snapshot } from './fixture';
import { requestDetail, traceRecord } from './trace-fixture';

afterEach(cleanup);
const failed = () => { const row = traceRecord(0, { state: 'failed' }); row.request!.failure_kind = 'timeout'; return row; };
const retry = () => { const row = traceRecord(1, { state: 'running' }); row.request!.previous_failure_kind = 'timeout'; row.request!.predecessor = { availability: 'available', request_id: 'request-0' }; return row; };

it('uses native ordinals and exact predecessor; clipped pages never invent missing schedules or failure ownership', () => {
  const a = failed(), b = retry();
  expect(modelRetryNotices([a], 'attempt-a')).toMatchObject([{ retry: 0, failure: { id: a.id } }]);
  expect(modelRetryNotices([a, b], 'attempt-a')).toMatchObject([{ retry: 1, request: { id: b.id }, failure: { id: a.id } }]);
  expect(modelRetryNotices([b], 'attempt-a')).toMatchObject([{ retry: 1, failure: undefined }]);
  expect(modelRetryNotices([a, b], 'other-attempt')).toEqual([]);
  b.request!.previous_failure_kind = 'context_window_exceeded';
  expect(modelRetryNotices([b], 'attempt-a')).toEqual([]);
  b.request!.previous_failure_kind = 'timeout'; b.request!.predecessor = { availability: 'available', request_id: 'unloaded' };
  expect(modelRetryNotices([a, b], 'attempt-a')[0].failure).toBeUndefined();
});

it('shows timeout then active retry, loads only disclosed failure details and stops animation on settlement or disconnect', async () => {
  const server = new Server();
  const state = snapshot(); state.attempt = { attempt_id: 'attempt-a', phase: { type: 'running' }, turn: 0 };
  state.trace = { records: [failed()] }; server.snapshots.set('A', state);
  const failureDetail = requestDetail(0); failureDetail.request!.failure = { kind: 'timeout', message: { text: 'Native stream idle timeout', truncated: false } };
  server.handlers.set('session/traceDetail', () => ({ type: 'trace_detail', detail: failureDetail }));
  await server.attached('A');
  try {
    const ui = render(<ModelRetries client={server.client} sessionId="A" attemptId="attempt-a"/>);
    expect(ui.container.querySelector('summary')?.textContent).toBe('Model request timed out');
    expect(ui.container.querySelector('[data-active]')).toBeNull();
    expect(server.socket.requests.filter(r => r.method === 'session/traceDetail')).toHaveLength(0);
    state.trace = { records: [failed(), retry()] }; server.snapshots.set('A', structuredClone(state));
    await act(() => server.client.refresh('A'));
    expect(ui.getByText('Retrying model request (1)')).toBeTruthy();
    expect(ui.container.querySelector('[data-shimmer="true"]')).toBeTruthy();
    await act(async () => { const details = ui.container.querySelector('details')!; details.open = true; fireEvent(details, new Event('toggle')); });
    await waitFor(() => expect(ui.getByText('Native stream idle timeout')).toBeTruthy());
    expect(server.socket.requests.filter(r => r.method === 'session/traceDetail')).toHaveLength(1);
    state.attempt!.phase = { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } };
    state.trace.records[1].state = 'completed'; server.snapshots.set('A', structuredClone(state));
    await act(() => server.client.refresh('A'));
    expect(ui.container.querySelector('[data-shimmer="true"]')).toBeNull();
    expect(ui.getByText('Model request retried (1)')).toBeTruthy();
    state.attempt!.phase = { type: 'running' }; state.trace.records[1].state = 'running';
    server.snapshots.set('A', structuredClone(state)); await act(() => server.client.refresh('A'));
    expect(ui.container.querySelector('[data-shimmer="true"]')).toBeTruthy();
    await act(async () => server.client.disconnect());
    expect(ui.container.querySelector('[data-shimmer="true"]')).toBeNull();
    expect(ui.getByText('Model request retried (1)')).toBeTruthy();
    expect(server.socket.requests.some(r => r.method === 'turn/start')).toBe(false);
  } finally { server.client.disconnect(); }
});
