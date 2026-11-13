import { describe, expect, it } from 'vitest';
import { nodeLineageIdentity, recoveredThreadStatusReads, supersededHistoryReads, type RequestTiming, type ThreadStatusTiming } from './session-latency';

describe('node lineage identity narrowing', () => {
  it('records only complete string identities', () => {
    expect(nodeLineageIdentity('session-a', { id: 'node-a', conversation_id: 'conversation-a' })).toEqual({
      conversationId: 'conversation-a',
      sessionId: 'session-a',
      nodeId: 'node-a',
    });
  });

  it.each([
    [undefined, { id: 'node-a', conversation_id: 'conversation-a' }],
    ['', { id: 'node-a', conversation_id: 'conversation-a' }],
    ['session-a', undefined],
    ['session-a', {}],
    ['session-a', { id: 'node-a' }],
    ['session-a', { conversation_id: 'conversation-a' }],
    ['session-a', { id: '', conversation_id: 'conversation-a' }],
    ['session-a', { id: 'node-a', conversation_id: '' }],
  ] as const)('rejects malformed or missing identity %#', (sessionId, node) => {
    expect(nodeLineageIdentity(sessionId, node)).toBeUndefined();
  });
});


const target = JSON.stringify(['session-a', 'conversation-a', 'runtime-1', 'attachment-1']);
const otherTarget = JSON.stringify(['session-a', 'conversation-a', 'runtime-1', 'attachment-2']);
function evidence(): RequestTiming[] {
  return [
    { method: 'session/history', historyAt: 'latest', sessionId: 'session-a', nodeId: 'node-a', start: 10, end: 30, bytes: 100, error: 'stale_runtime' },
    { method: 'thread/resume', resultType: 'attached', sessionId: 'session-a', nodeId: 'node-a', target, start: 20, end: 40 },
    { method: 'session/checkpoint', resultType: 'checkpoint', target, start: 40, end: 50 },
    { method: 'session/subscribe', resultType: 'subscribed', target, start: 50, end: 60 },
  ];
}

describe('latency evidence for a cold preview superseded by attachment', () => {
  it('returns the original refused history row only after exact live observation succeeds', () => {
    const rows = evidence();
    const original = structuredClone(rows);
    expect(supersededHistoryReads(rows)).toEqual([rows[0]]);
    expect(supersededHistoryReads(rows)[0]).toBe(rows[0]);
    expect(rows).toEqual(original);
    expect(rows[0].error).toBe('stale_runtime');
  });

  it.each(['older', 'newer', 'message', '', undefined])('does not exempt a %s history request', historyAt => {
    const rows = evidence(); rows[0].historyAt = historyAt;
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it.each([1, 2, 3])('requires the native success receipt for handoff step %i', index => {
    for (const resultType of [undefined, '', 'wrong_receipt']) {
      const rows = evidence(); rows[index].resultType = resultType;
      expect(supersededHistoryReads(rows)).toEqual([]);
    }
  });

  it('uses measured times rather than evidence array ordering', () => {
    const rows = evidence();
    expect(supersededHistoryReads([rows[3], rows[0], rows[2], rows[1]])).toEqual([rows[0]]);
  });

  it.each([
    [0, 'sessionId'], [0, 'nodeId'], [1, 'sessionId'], [1, 'nodeId'],
    [1, 'target'], [2, 'target'], [3, 'target'],
  ] as const)('rejects missing or empty identity %i.%s', (index, field) => {
    for (const value of [undefined, '']) {
      const rows = evidence();
      rows[index][field] = value;
      expect(supersededHistoryReads(rows)).toEqual([]);
    }
  });

  it.each([
    [1, 'sessionId', 'session-b'], [1, 'nodeId', 'node-b'],
    [1, 'target', otherTarget], [2, 'target', otherTarget], [3, 'target', otherTarget],
  ] as const)('rejects foreign identity %i.%s', (index, field, value) => {
    const rows = evidence();
    rows[index][field] = value;
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it.each([0, 1, 2, 3])('requires complete valid timing for row %i', index => {
    for (const patch of [
      { start: undefined }, { end: undefined }, { start: Number.NaN }, { end: Number.NaN },
      { start: Number.POSITIVE_INFINITY }, { end: Number.POSITIVE_INFINITY },
      { start: -1 }, { end: -1 }, { end: evidence()[index].start - 1 },
    ]) {
      const rows = evidence();
      Object.assign(rows[index], patch);
      expect(supersededHistoryReads(rows)).toEqual([]);
    }
  });

  it.each([
    [0, 19], [0, 20], [40, 50], [41, 50],
  ])('requires history interval %i–%i to overlap in-flight attachment', (start, end) => {
    const rows = evidence();
    Object.assign(rows[0], { start, end });
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it.each([
    [2, 39], [3, 49],
  ])('rejects a read dispatched before its predecessor completes (%i)', (index, start) => {
    const rows = evidence();
    rows[index].start = start;
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it.each([1, 2, 3])('requires successful attach/checkpoint/subscribe row %i', index => {
    for (const error of ['stale_runtime', 'operation_failed', '']) {
      const rows = evidence();
      rows[index].error = error;
      expect(supersededHistoryReads(rows)).toEqual([]);
    }
  });

  it.each([1, 2, 3])('rejects an incomplete observation chain missing row %i', index => {
    const rows = evidence();
    rows.splice(index, 1);
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it.each(['read_cut_expired', 'operation_failed', 'unknown_session', undefined])('does not allow a history result classified %s', error => {
    const rows = evidence();
    rows[0].error = error;
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it.each(['thread/fork', 'session/branch', 'session/switchNode', 'inbound/queue', 'thread/resume', 'session/checkpoint', 'session/subscribe'])('never classifies a failed %s as a superseded preview', method => {
    const rows = evidence();
    const failure = { ...rows[0], method };
    rows.push(failure);
    const superseded = new Set(supersededHistoryReads(rows));
    expect(rows.filter(row => row.error !== undefined && !superseded.has(row))).toEqual([failure]);
  });

  it('cannot combine checkpoint and subscription from separate attachment incarnations', () => {
    const rows = evidence();
    rows[3].target = otherTarget;
    rows.push({ ...rows[1], target: otherTarget, start: 31, end: 39 });
    expect(supersededHistoryReads(rows)).toEqual([]);
  });

  it('keeps other refused history rows even when one preview has been superseded', () => {
    const rows = evidence();
    rows.push({ ...rows[0], nodeId: 'node-b' });
    expect(supersededHistoryReads(rows)).toEqual([rows[0]]);
  });

  it('accepts a Retry/switchNode handoff once resume carries the selected Session and Node', () => {
    // Mirrors browser evidence after session/branch + session/switchNode: the
    // observer must annotate thread/resume from the switchNode Conversation
    // owner, because resume itself only names threadId on the wire.
    const rows = evidence();
    rows[0] = { ...rows[0], sessionId: 'ses-retry', nodeId: 'node-retry', start: 100, end: 150 };
    rows[1] = {
      ...rows[1],
      sessionId: 'ses-retry',
      nodeId: 'node-retry',
      threadId: 'conv-retry',
      target: JSON.stringify(['ses-retry', 'conv-retry', '2', 'attachment-1']),
      start: 120,
      end: 200,
    };
    rows[2] = { ...rows[2], target: rows[1].target, start: 200, end: 210 };
    rows[3] = { ...rows[3], target: rows[1].target, start: 210, end: 220 };
    expect(supersededHistoryReads(rows)).toEqual([rows[0]]);
  });
});

describe('latency evidence for recovered native status capture', () => {
  function evidence(): RequestTiming[] {
    return [
      { method: 'thread/read', connection: 1, threadId: 'thread-a', start: 10, end: 20, error: 'observation_unavailable' },
      { method: 'thread/read', connection: 1, threadId: 'thread-a', resultThreadId: 'thread-a', resultType: 'thread', start: 20, end: 30 },
    ];
  }

  const notification = (time: number, revision = '5'): Extract<ThreadStatusTiming, { entry: unknown }> => ({
    kind: 'notification', connection: 1, time,
    entry: { threadId: 'thread-a', revision, status: { type: 'idle' } },
  });

  it('accepts a newer same-connection notification after the failed read, preserving the refusal', () => {
    const rows = evidence().slice(0, 1), original = structuredClone(rows);
    for (const time of [21, 30]) {
      const statuses = [notification(5, '4'), notification(time)];
      expect(recoveredThreadStatusReads(rows, statuses)).toEqual([rows[0]]);
      expect(recoveredThreadStatusReads(rows, statuses)[0]).toBe(rows[0]);
    }
    expect(rows).toEqual(original);
  });

  it('rejects stale, foreign, early and malformed notification evidence', () => {
    const rows = evidence().slice(0, 1);
    for (const event of [
      notification(5), notification(10), notification(15), notification(20), notification(21, '4'), notification(21, '3'),
      notification(21, 'invalid'), { ...notification(21), connection: 2 },
      { ...notification(21), time: Number.NaN }, { ...notification(21), connection: 0 },
      { ...notification(21), entry: { threadId: 'other', revision: '5', status: { type: 'idle' as const } } },
      { ...notification(21), kind: 'snapshot' as const },
    ] satisfies ThreadStatusTiming[]) {
      expect(recoveredThreadStatusReads(rows, [notification(1, '4'), event])).toEqual([]);
    }
  });

  it('uses snapshot floors and accepts new notification proof after a resync', () => {
    const rows = evidence().slice(0, 1);
    const snapshot: ThreadStatusTiming = { ...notification(5, '8'), kind: 'snapshot' };
    expect(recoveredThreadStatusReads(rows, [snapshot, notification(21, '7')])).toEqual([]);
    expect(recoveredThreadStatusReads(rows, [snapshot, notification(21, '9')])).toEqual([rows[0]]);
    expect(recoveredThreadStatusReads(rows, [notification(15), { kind: 'resync', connection: 1, time: 16 }])).toEqual([]);
    expect(recoveredThreadStatusReads(rows, [notification(21), { kind: 'resync', connection: 1, time: 15 }])).toEqual([rows[0]]);
    expect(recoveredThreadStatusReads(rows, [notification(15), { kind: 'resync', connection: 1, time: 12 }])).toEqual([]);
    expect(recoveredThreadStatusReads(rows, [notification(21), { kind: 'resync', connection: 2, time: 15 }])).toEqual([rows[0]]);
    expect(recoveredThreadStatusReads(rows, [notification(21), { kind: 'resync', connection: 1, time: 5 }])).toEqual([rows[0]]);
  });

  it('retains the original error and only accepts a later strong read of the same Thread and connection', () => {
    const rows = evidence(), original = structuredClone(rows);
    expect(recoveredThreadStatusReads([rows[1], rows[0]])).toEqual([rows[0]]);
    expect(recoveredThreadStatusReads(rows)[0]).toBe(rows[0]);
    expect(rows).toEqual(original);
    expect(recoveredThreadStatusReads(rows.slice(0, 1))).toEqual([]);
    const other = { ...rows[0], threadId: 'thread-b' };
    expect(recoveredThreadStatusReads([...rows, other])).toEqual([rows[0]]);
  });

  it.each([0, 1])('requires a complete valid read interval for row %i', index => {
    for (const patch of [
      { start: Number.NaN }, { end: undefined }, { end: Number.POSITIVE_INFINITY },
      { start: -1 }, { end: evidence()[index].start - 1 },
    ]) {
      const rows = evidence(); Object.assign(rows[index], patch);
      expect(recoveredThreadStatusReads(rows)).toEqual([]);
    }
  });

  it('rejects earlier and overlapping recovery reads', () => {
    for (const interval of [{ start: 0, end: 9 }, { start: 19, end: 30 }]) {
      const rows = evidence(); Object.assign(rows[1], interval);
      expect(recoveredThreadStatusReads(rows)).toEqual([]);
    }
  });

  it('requires a strong recovery read to finish within its connection epoch', () => {
    const rows = evidence();
    expect(recoveredThreadStatusReads(rows, [{ kind: 'resync', connection: 1, time: 25 }])).toEqual([]);
    expect(recoveredThreadStatusReads(rows, [{ kind: 'resync', connection: 2, time: 25 }])).toEqual([rows[0]]);
    expect(recoveredThreadStatusReads(rows, [{ kind: 'resync', connection: 1, time: 19 }])).toEqual([rows[0]]);
  });

  it.each([0, 1])('requires the exact connection and request Thread identity for row %i', index => {
    for (const patch of [
      { connection: undefined }, { connection: 0 }, { connection: -1 }, { connection: 1.5 },
      { connection: Number.MAX_SAFE_INTEGER + 1 }, { connection: 2 },
      { threadId: undefined }, { threadId: '' }, { threadId: 'thread-b' },
    ]) {
      const rows = evidence(); Object.assign(rows[index], patch);
      expect(recoveredThreadStatusReads(rows)).toEqual([]);
    }
  });

  it('requires a successful exact Thread response, without excusing other methods or errors', () => {
    for (const patch of [
      { resultThreadId: undefined }, { resultThreadId: '' }, { resultThreadId: 'thread-b' },
      { resultType: undefined }, { resultType: 'sessions' },
      { error: 'observation_unavailable' }, { error: '' }, { method: 'session/list' },
    ]) {
      const rows = evidence(); Object.assign(rows[1], patch);
      expect(recoveredThreadStatusReads(rows)).toEqual([]);
    }
    for (const patch of [
      { error: undefined }, { error: 'operation_failed' }, { error: 'stale_runtime' },
      { method: 'session/history' }, { method: 'inbound/queue' }, { method: 'thread/resume' },
    ]) {
      const rows = evidence(); Object.assign(rows[0], patch);
      expect(recoveredThreadStatusReads(rows)).toEqual([]);
    }
  });
});
