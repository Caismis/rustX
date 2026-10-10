import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { modelRetryNotices, modelRetryPlacements } from '../src/bindings/model-retry';
import { useModelRetryFeedback } from '../src/app/agent/ModelRetry';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import type { AppServerClient } from '../src/client/app-server';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v42';
import { Server, snapshot } from './fixture';
import { requestDetail, traceRecord } from './trace-fixture';

afterEach(cleanup);
const failed = () => { const row = traceRecord(0, { state: 'failed' }); row.request!.failure_kind = 'timeout'; return row; };
const retry = () => { const row = traceRecord(1, { state: 'running' }); row.request!.previous_failure_kind = 'timeout'; row.request!.predecessor = { availability: 'available', request_id: 'request-0' }; return row; };
function RetryFeedback({ client, sessionId, attemptId, entries = [] }: { client: AppServerClient; sessionId: string; attemptId: string; entries?: RuntimeClientTranscriptEntry[] }) {
  const feedback = useModelRetryFeedback(client, sessionId, entries);
  return <>{feedback.attempts.get(attemptId)}{[...feedback.messages.values()]}</>;
}

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
    const ui = render(<RetryFeedback client={server.client} sessionId="A" attemptId="attempt-a"/>);
    expect(ui.container.querySelector('summary')?.textContent).toBe('Model request timed out');
    expect(ui.container.querySelector('[data-active]')).toBeNull();
    expect(server.socket.requests.filter(r => r.method === 'session/traceDetail')).toHaveLength(0);
    state.attempt!.in_flight = { message_id: retry().request!.assistant_message_id, blocks: [] };
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

it('exact native publication ownership activates an earlier incomplete retry without a Trace reread', async () => {
 const server = new Server(), state = snapshot(), request = retry();request.state='incomplete';
 state.attempt={attempt_id:'attempt-a',phase:{type:'running'},turn:0};
 state.trace={records:[failed(),request]};server.snapshots.set('A',state);await server.attached('A');
 try {
  const ui=render(<RetryFeedback client={server.client} sessionId="A" attemptId="attempt-a"/>);
  expect(ui.container.querySelector('[data-active]')).toBeNull();
  const publish=async(message_id:string)=>act(async()=>{
   server.cursor++;server.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:server.target('A'),cursor:String(server.cursor),event:{type:'assistant_message_started',attempt_id:'attempt-a',message_id}}});
  });
  await publish('another-request');expect(ui.container.querySelector('[data-active]')).toBeNull();
  await publish(request.request!.assistant_message_id);expect(ui.getByText('Retrying model request (1)')).toBeTruthy();
  expect(server.requests.filter(row=>row.request.method==='session/trace')).toHaveLength(0);
 } finally {server.client.disconnect();}
});

it('places each native retry chain before its first retained publication without borrowing an adjacent message', () => {
  const a = failed(), b = retry(), c = traceRecord(2), d = retry();
  c.request!.previous_failure_kind = 'timeout';
  d.id = 'another-step'; d.location.step_id = '2'; d.request!.assistant_message_id = 'step-two-reply';
  const records = [a, b, c, d];
  expect(modelRetryPlacements(records, new Set(['message-1', 'message-2', 'step-two-reply'])).map(row => row.beforeMessageId)).toEqual(['message-1', 'step-two-reply']);
  expect(modelRetryPlacements(records, new Set(['message-2', 'unrelated-reply'])).map(row => row.beforeMessageId)).toEqual(['message-2', undefined]);
  expect(modelRetryPlacements([b], new Set(['unrelated-reply']))[0].beforeMessageId).toBeUndefined();
  const recovery = traceRecord(1), timeout = traceRecord(2), restarted = traceRecord(3);
  recovery.request!.previous_failure_kind = 'context_window_exceeded'; timeout.request!.failure_kind = 'timeout'; restarted.request!.previous_failure_kind = 'timeout';
  expect(modelRetryPlacements([recovery, timeout, restarted], new Set(['message-1', 'message-3']))[0].beforeMessageId).toBe('message-3');
});

it.each(['message', 'audit'] as const)('retains independent collapsed retry records at their own %s after completion, process expansion and reattachment', async publication => {
  const server = new Server(), state = snapshot();
  const first = retry(), second = retry();
  first.state = second.state = 'completed'; second.id = 'step-two-retry'; second.location.step_id = '2'; second.request!.assistant_message_id = 'step-two-reply';
  second.request!.request_id = 'step-two-request-1'; second.request!.predecessor = { availability: 'available', request_id: 'step-two-request-0' };
  state.trace = { records: [failed(), first, second] };
  state.attempt = { attempt_id: 'attempt-a', phase: { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } }, turn: 0 };
  const process = { conversation_id: state.conversation_id, attempt_id: 'attempt-a', control_cursor: '1', final_message_id: 'final-reply', outcome: 'completed' as const, tool_call_count: 0, message_count: 3 };
  state.transcript = { entries: [
    { cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'before-retry', content: [{ type: 'text', text: 'Earlier reply' }] } }, turn_process: process },
    { cursor: '2', item: { type: 'message', message: { role: 'assistant', id: 'message-1', content: [{ type: 'text', text: 'First recovered reply' }] } }, turn_process: process },
    { cursor: '3', item: { type: 'message', message: { role: 'assistant', id: 'step-two-reply', content: [{ type: 'reasoning', text: 'Second recovered reasoning' }] } }, turn_process: process },
    { cursor: '4', item: { type: 'message', message: { role: 'assistant', id: 'final-reply', content: [{ type: 'text', text: 'Final reply' }] } }, turn_process: process },
  ] };
  if (publication === 'audit') state.transcript.entries![1].item = { type: 'publication_audit', audit: {
    stream_id: 'retry-stream', attempt_id: 'attempt-a', turn_id: '1', request_id: 'request-1', message_id: 'message-1',
    kind: 'incomplete', content: [{ kind: 'text', block_index: 0, text: 'First recovered reply' }], settled_at: '2026-10-10T00:00:00Z',
  } };
  server.snapshots.set('A', state); await server.attached('A');
  try {
    const ui = render(<ConversationLive client={server.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/>);
    const notices = () => [...ui.container.querySelectorAll<HTMLDetailsElement>('[data-model-retry]')];
    expect(notices()).toHaveLength(2);
    const anchors = notices().map(row => row.closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key'));
    expect(anchors).toEqual([publication === 'audit' ? 'publication:2' : 'message:message-1', 'message:step-two-reply']);
    for (const row of notices()) { expect(row.open).toBe(false); expect(row.closest('[hidden]')).toBeNull(); expect(row.hasAttribute('data-active')).toBe(false); }
    const firstReply = ui.getByText('First recovered reply');
    expect(notices()[0].compareDocumentPosition(firstReply) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // A retry is an independent row even when its reasoning body is collapsed.
    expect(ui.getByText('Second recovered reasoning').closest('[hidden]')).toBeTruthy();
    await act(async () => fireEvent.click(ui.container.querySelector<HTMLButtonElement>('[data-turn-process]')!));
    expect(notices().map(row => row.closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key'))).toEqual(anchors);
    expect(notices().every(row => !row.closest('[hidden]'))).toBe(true);
    await act(async () => server.client.release('A'));
    await act(async () => server.client.attach('A'));
    expect(notices()).toHaveLength(2);
    expect(notices().map(row => row.closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key'))).toEqual(anchors);
    expect(notices().every(row => !row.open && !row.hasAttribute('data-active'))).toBe(true);
    expect(server.requests.some(row => row.request.method === 'turn/start')).toBe(false);
  } finally { server.client.disconnect(); }
});

it('keeps an opened failure disclosure when native publication moves its retry into the message stream', async () => {
  const server = new Server(), state = snapshot();
  state.attempt = { attempt_id: 'attempt-a', phase: { type: 'running' }, turn: 0, in_flight: { message_id: 'message-1', blocks: [] } };
  state.trace = { records: [failed(), retry()] }; server.snapshots.set('A', state);
  server.handlers.set('session/traceDetail', () => ({ type: 'trace_detail', detail: requestDetail(0) }));
  await server.attached('A');
  try {
    const ui = render(<RetryFeedback client={server.client} sessionId="A" attemptId="attempt-a"/>);
    await act(async () => { const row = ui.container.querySelector('details')!; row.open = true; fireEvent(row, new Event('toggle')); });
    const entries: RuntimeClientTranscriptEntry[] = [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'message-1', content: [{ type: 'text', text: 'Recovered' }] } } }];
    await act(async () => ui.rerender(<RetryFeedback client={server.client} sessionId="A" attemptId="attempt-a" entries={entries}/>));
    expect(ui.container.querySelector('details')?.open).toBe(true);
    expect(ui.container.querySelectorAll('[data-model-retry]')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'session/traceDetail')).toHaveLength(1);
  } finally { server.client.disconnect(); }
});
