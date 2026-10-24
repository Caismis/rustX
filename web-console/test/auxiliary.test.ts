import { afterEach, expect, it, vi } from 'vitest';
import { Server } from './fixture';
import type { AuxiliaryResult, AuxiliarySegment } from '../../protocol/app-server/v57';
const servers: Server[] = [];
afterEach(() => { for (const server of servers.splice(0)) server.client.disconnect(); vi.useRealTimers(); });
async function connected() {
  const server = new Server(); server.capabilities = { ...server.capabilities, delivery_access: true }; servers.push(server); await server.attached('A');
  server.handlers.set('delivery/cancel', () => ({ type: 'delivery_cancel', accepted: true }));
  server.handlers.set('auxiliary/btw', request => ({ type: 'auxiliary_submitted', receipt: { call_id: 'call-A', correlation: request.method === 'auxiliary/btw' ? request.params.submission.correlation : '', scope: 'scope-A' } }));
  server.handlers.set('auxiliary/status', () => ({ type: 'auxiliary_status', status: { call_id: 'call-A', executing: true, inference_stopped: false, settlement_confirmed: false, outcome: null, result: 'pending' } }));
  return server;
}
it('one explicit question survives panel close without another submission or cancellation', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  vi.useFakeTimers();
  await questions.submit('A', 'What is committed?');
  const close = questions.observe('A');
  await questions.submit('A', 'Repeated activation');
  close();
  await vi.advanceTimersByTimeAsync(10_000);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(0);
  expect(questions.snapshot('A')?.correlation).toBeTruthy();
});
it('attempt reset invalidates provisional output and a complete retained result repairs gaps', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  vi.useFakeTimers(); await questions.submit('A', 'Question'); const close = questions.observe('A');
  const target = server.client.target('A');
  const segment = (value: AuxiliarySegment) => questions.segment(target, value);
  segment({ kind: 'reset', call_id: 'call-A', request_id: 'request-1', attempt: 1 });
  segment({ kind: 'text', call_id: 'call-A', request_id: 'request-1', attempt: 1, start: '0', end: '3', text: 'old' });
  segment({ kind: 'reset', call_id: 'call-A', request_id: 'request-2', attempt: 2 });
  expect(questions.snapshot('A')).toMatchObject({ text: '', phase: 'retrying' });
  segment({ kind: 'text', call_id: 'call-A', request_id: 'request-2', attempt: 2, start: '5', end: '8', text: 'gap' });
  expect(questions.snapshot('A')?.phase).toBe('incomplete');
  const result: AuxiliaryResult = { call_id: 'call-A', final_request_id: 'request-2', execution: 'completed', value: { kind: 'btw', text: 'A complete answer' } };
  server.handlers.set('auxiliary/status', () => ({ type: 'auxiliary_status', status: { call_id: 'call-A', executing: false, inference_stopped: true, settlement_confirmed: true, outcome: 'completed', result: 'retained' } }));
  server.handlers.set('auxiliary/result', () => ({ type: 'auxiliary_result', result }));
  await vi.advanceTimersByTimeAsync(0);
  await questions.reconcile('A');
  segment({ kind: 'reset', call_id: 'call-A', request_id: 'stale', attempt: 3 });
  expect(questions.snapshot('A')).toMatchObject({ phase: 'complete', text: 'A complete answer', request: 'request-2', complete: true });
  close();
});
it('closing or replacing the view rejects late text and cancel uses the reserved control lane', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  vi.useFakeTimers(); await questions.submit('A', 'Question'); const close = questions.observe('A'); close();
  questions.segment(server.client.target('A'), { kind: 'reset', call_id: 'call-A', request_id: 'late', attempt: 1 });
  expect(questions.snapshot('A')?.request).toBeUndefined();
  server.held.add('agent/wait');
  const waits = Array.from({ length: 6 }, (_, index) => server.client.request({ method: 'agent/wait', params: { target: server.client.target('A'), agent_id: `agent-${index}` } }, 'agent_wait'));
  for (const wait of waits) void wait.catch(() => {});
  server.handlers.set('auxiliary/cancel', () => ({ type: 'auxiliary_cancelled', accepted: true }));
  await questions.cancel('A');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(1);
});
it('uncertain correlation is queried read-only and never replayed', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  server.handlers.set('auxiliary/submissionLookup', () => ({ type: 'auxiliary_submission_lookup', lookup: { state: 'unresolved' } }));
  server.held.add('auxiliary/btw');
  const submit = questions.submit('A', 'Keep exact bytes  ');
  await server.waitFor('auxiliary/btw', 1);
  server.client.disconnect(); await submit;
  expect(questions.snapshot('A')?.phase).toBe('uncertain');
  await server.connect();
  await questions.reconcile('A');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/submissionLookup')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
});
it('read failure stops automatic polling while exact cancellation remains available', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  vi.useFakeTimers(); await questions.submit('A', 'Question');
  server.handlers.set('auxiliary/status', () => { throw new Error('source unavailable'); });
  // An explicit error response models an unavailable source without changing
  // controller authority; the cancellation lane never awaits that source.
  server.held.add('auxiliary/status');
  const close = questions.observe('A'); await vi.advanceTimersByTimeAsync(0);
  const pending = server.requests.filter(row => row.request.method === 'auxiliary/status').at(-1)!;
  pending.socket.deliver({ jsonrpc: '2.0', id: pending.request.id, error: { code: -32000, message: 'source unavailable', data: { kind: 'content_source_unavailable' } } });
  await vi.advanceTimersByTimeAsync(10_000);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/status')).toHaveLength(1);
  expect(questions.snapshot('A')?.phase).toBe('incomplete');
  server.handlers.set('auxiliary/cancel', () => ({ type: 'auxiliary_cancelled', accepted: true }));
  await questions.cancel('A'); close();
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(1);
});
it('repeated check actions share one physically pending read and close aborts it', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  vi.useFakeTimers(); await questions.submit('A', 'Question'); server.held.add('auxiliary/status');
  const close = questions.observe('A');
  const reads = Array.from({ length: 32 }, () => questions.reconcile('A'));
  expect(server.requests.filter(row => row.request.method === 'auxiliary/status')).toHaveLength(1);
  close();
  const pending = server.requests.filter(row => row.request.method === 'auxiliary/status').at(-1)!;
  pending.socket.deliver({ jsonrpc: '2.0', id: pending.request.id, error: { code: -32000, message: 'cancelled', data: { kind: 'delivery_cancelled' } } });
  await Promise.all(reads);
  expect(server.requests.filter(row => row.request.method === 'delivery/cancel')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(0);
});
it('a delayed original receipt cannot regress a final result recovered by lookup', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw');
  const submit = questions.submit('A', 'Question');
  await server.waitFor('auxiliary/btw', 1);
  const original = server.requests.filter(row => row.request.method === 'auxiliary/btw')[0]!;
  const receipt = { call_id: 'call-A', correlation: questions.snapshot('A')!.correlation, scope: 'scope-A' };
  server.handlers.set('auxiliary/submissionLookup', () => ({ type: 'auxiliary_submission_lookup', lookup: { state: 'known', receipt } }));
  server.handlers.set('auxiliary/status', () => ({ type: 'auxiliary_status', status: { call_id: 'call-A', executing: false, inference_stopped: true, settlement_confirmed: true, outcome: 'completed', result: 'retained' } }));
  server.handlers.set('auxiliary/result', () => ({ type: 'auxiliary_result', result: { call_id: 'call-A', final_request_id: 'request-A', execution: 'completed', value: { kind: 'btw', text: 'Complete first' } } }));
  await questions.reconcile('A');
  original.socket.success(original.request, { type: 'auxiliary_submitted', receipt }); await submit;
  expect(questions.snapshot('A')).toMatchObject({ phase: 'complete', complete: true, text: 'Complete first' });
});
it.each(['cancelled', 'invalid_output'] as const)('a retained %s result fences queued resets and text', async reason => {
  const server = await connected(), questions = server.client.auxiliary;
  vi.useFakeTimers(); await questions.submit('A', 'Question');
  server.handlers.set('auxiliary/status', () => ({ type: 'auxiliary_status', status: { call_id: 'call-A', executing: false, inference_stopped: true, settlement_confirmed: true, outcome: 'failed', result: 'retained' } }));
  server.handlers.set('auxiliary/result', () => ({ type: 'auxiliary_result', result: { call_id: 'call-A', final_request_id: 'request-A', execution: 'failed', value: { kind: 'rejected', reason } } }));
  const close = questions.observe('A'); await vi.advanceTimersByTimeAsync(0);
  questions.segment(server.client.target('A'), { kind: 'reset', call_id: 'call-A', request_id: 'late', attempt: 2 });
  expect(questions.snapshot('A')).toMatchObject({ phase: reason === 'cancelled' ? 'cancelled' : 'failed', settled: true, complete: false }); close();
});
it('a pending native CallId never becomes a confirmed receipt before preparation succeeds', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw');
  const submit = questions.submit('A', 'Question'); await server.waitFor('auxiliary/btw', 1);
  server.handlers.set('auxiliary/submissionLookup', () => ({ type: 'auxiliary_submission_lookup', lookup: { state: 'pending', scope: 'scope-A', call_id: 'call-A' } }));
  await questions.reconcile('A');
  expect(questions.snapshot('A')).toMatchObject({ phase: 'preparing', pendingCall: 'call-A' });
  expect(questions.snapshot('A')?.receipt).toBeUndefined();
  const original = server.requests.filter(row => row.request.method === 'auxiliary/btw')[0]!;
  original.socket.deliver({ jsonrpc: '2.0', id: original.request.id, error: { code: -32000, message: 'preparation failed', data: { kind: 'auxiliary', reason: 'unavailable' } } });
  await submit; expect(questions.snapshot('A')?.phase).toBe('failed');
  server.handlers.set('auxiliary/submissionLookup', () => ({ type: 'auxiliary_submission_lookup', lookup: { state: 'confirmed_not_admitted', scope: 'scope-A' } }));
  await questions.reconcile('A'); expect(questions.snapshot('A')).toMatchObject({ phase: 'failed', settled: true });
  expect(server.requests.filter(row => row.request.method === 'auxiliary/status')).toHaveLength(0);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
});

it('does not offer execution without a native Delivery Access grant', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  expect(server.client.hasAuxiliaryAuthority('A')).toBe(false);
  await expect(server.client.auxiliary.submit('A', 'Question')).rejects.toThrow();
  expect(server.requests.some(row => row.request.method === 'auxiliary/btw')).toBe(false);
});

it('publication refusal after native admission retains correlation and forbids a second question across reconnect', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw');
  const submit = questions.submit('A', 'Original question');
  const original = await server.waitFor('auxiliary/btw', 1);
  const correlation = questions.snapshot('A')!.correlation;
  server.socket.deliver({ jsonrpc: '2.0', id: original.id, error: { code: -32000, message: 'Receipt publication revoked after admission', data: { kind: 'outcome_unknown' } } });
  await submit;
  expect(questions.snapshot('A')).toMatchObject({ phase: 'uncertain', correlation });
  await questions.submit('A', 'Must not replace original');
  server.client.disconnect(); await server.connect();
  const receipt = { correlation, call_id: 'call-A', scope: 'scope-A' };
  server.handlers.set('auxiliary/submissionLookup', () => ({ type: 'auxiliary_submission_lookup', lookup: { state: 'known', receipt } }));
  await questions.reconcile('A');
  await questions.submit('A', 'Still running');
  expect(questions.snapshot('A')).toMatchObject({ correlation, receipt, phase: 'preparing' });
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
  server.handlers.set('auxiliary/cancel', () => ({ type: 'auxiliary_cancelled', accepted: true }));
  await questions.cancel('A');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel').at(-1)?.request.params).toMatchObject({ call_id: 'call-A' });
});

it.each([false, true])('exact auxiliary cancel survives a real SourceUnavailable notification (checkpoint held=%s)', async held => {
  const server = await connected(), questions = server.client.auxiliary;
  await questions.submit('A', 'Question');
  const target = server.target('A'), before = server.client.getSnapshot().views.A;
  const registration = server.registrations.get(JSON.stringify(target))!;
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id: registration, message: 'Original source failed' } });
  expect(server.client.getSnapshot().views.A.observationState).toBe('unavailable');
  expect(server.client.isAttachmentControlCurrent('A', before.attachmentObservation)).toBe(false);
  expect(server.client.hasAuxiliaryAuthority('A')).toBe(true);
  let refresh: Promise<unknown> | undefined;
  if (held) {
    server.held.add('session/checkpoint');
    refresh = server.client.refresh('A'); void refresh.catch(() => {});
    await server.waitFor('session/checkpoint', 2);
  }
  server.handlers.set('auxiliary/cancel', () => ({ type: 'auxiliary_cancelled', accepted: true }));
  await questions.cancel('A');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(1);
  expect(server.client.getSnapshot().views.A.cursor).toBe(before.cursor);
  expect(server.client.getSnapshot().views.A.snapshot).toBe(before.snapshot);
  if (held) { server.client.disconnect(); await refresh?.catch(() => {}); }
});

it('a decoded receipt survives connection loss during response pumping and prevents a second Call', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw'); server.held.add('session/list');
  const submit = questions.submit('A', 'Original question');
  const original = await server.waitFor('auxiliary/btw', 1);
  const correlation = questions.snapshot('A')!.correlation;
  // Five reads fill the other RPC slots; the sixth is queued. Releasing the
  // submission slot must pump it before the original Promise continuation.
  const before = server.requests.filter(row => row.request.method === 'session/list').length;
  const reads = Array.from({ length: 6 }, () => server.client.request({ method: 'session/list', params: { offset: 0, limit: 10 } }, 'sessions').catch(() => {}));
  expect(server.requests.filter(row => row.request.method === 'session/list')).toHaveLength(before + 5);
  const receipt = { correlation, call_id: 'call-A', scope: 'scope-A' };
  const generation = server.client.getSnapshot().generation;
  server.socket.readyState = 2;
  server.socket.success(original, { type: 'auxiliary_submitted', receipt });
  expect(server.client.getSnapshot().generation).not.toBe(generation);
  await submit; await Promise.all(reads);
  expect(questions.snapshot('A')).toMatchObject({ correlation, receipt, phase: 'preparing' });
  server.held.delete('session/list');
  await server.connect();
  await questions.submit('A', 'Do not duplicate');
  await questions.reconcile('A');
  expect(questions.snapshot('A')).toMatchObject({ correlation, receipt });
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
});

it.each(['pending', 'unresolved'] as const)('late %s lookup cannot reopen a definitely rejected submission', async state => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw'); server.held.add('auxiliary/submissionLookup');
  const submit = questions.submit('A', 'Invalid original');
  const original = await server.waitFor('auxiliary/btw', 1);
  const reading = questions.reconcile('A');
  const lookup = await server.waitFor('auxiliary/submissionLookup', 1);
  expect(lookup.params).toMatchObject({ scope: null });
  server.socket.deliver({ jsonrpc: '2.0', id: original.id, error: { code: -32000, message: 'preparation failed', data: { kind: 'auxiliary', reason: 'unavailable' } } });
  await submit;
  expect(questions.snapshot('A')?.phase).toBe('failed');
  server.socket.success(lookup, { type: 'auxiliary_submission_lookup', lookup: state === 'pending' ? { state, scope: 'scope-A', call_id: 'call-A' } : { state } });
  await reading;
  expect(questions.snapshot('A')).toMatchObject({ phase: 'failed', settled: true });
  server.held.delete('auxiliary/btw');
  await questions.submit('A', 'Corrected question');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(2);
});

it('manual reconciliation cannot reopen a definitely rejected submission', async () => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw');
  const submit = questions.submit('A', 'Invalid original');
  const original = await server.waitFor('auxiliary/btw', 1);
  server.socket.deliver({ jsonrpc: '2.0', id: original.id, error: { code: -32000, message: 'preparation failed', data: { kind: 'auxiliary', reason: 'unavailable' } } });
  await submit;
  server.handlers.set('auxiliary/submissionLookup', () => ({ type: 'auxiliary_submission_lookup', lookup: { state: 'unresolved' } }));
  await questions.reconcile('A');
  expect(questions.snapshot('A')).toMatchObject({ phase: 'failed', settled: true });
  expect(server.requests.filter(row => row.request.method === 'auxiliary/submissionLookup')).toHaveLength(0);
  server.held.delete('auxiliary/btw'); await questions.submit('A', 'Corrected question');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(2);
});

it.each(['pending', 'unresolved', 'confirmed_not_admitted'] as const)('late %s lookup cannot downgrade a confirmed receipt', async state => {
  const server = await connected(), questions = server.client.auxiliary;
  server.held.add('auxiliary/btw'); server.held.add('auxiliary/submissionLookup');
  const submit = questions.submit('A', 'Original');
  const original = await server.waitFor('auxiliary/btw', 1);
  const reading = questions.reconcile('A');
  const lookup = await server.waitFor('auxiliary/submissionLookup', 1);
  server.reply(original); await submit;
  const receipt = questions.snapshot('A')!.receipt;
  server.socket.success(lookup, { type: 'auxiliary_submission_lookup', lookup: state === 'unresolved' ? { state } : { state, scope: 'scope-A' } });
  await reading;
  expect(questions.snapshot('A')).toMatchObject({ receipt, phase: 'preparing' });
  expect(questions.snapshot('A')?.settled).not.toBe(true);
  await questions.submit('A', 'Do not duplicate');
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
});
