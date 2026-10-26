import { describe, expect, it } from 'vitest';
import { supersededHistoryReads, type RequestTiming } from './session-latency';

const target = JSON.stringify(['session-a', 'conversation-a', 'runtime-1', 'attachment-1']);
const otherTarget = JSON.stringify(['session-a', 'conversation-a', 'runtime-1', 'attachment-2']);
function evidence(): RequestTiming[] {
  return [
    { method: 'session/history', historyAt: 'latest', sessionId: 'session-a', nodeId: 'node-a', start: 10, end: 30, bytes: 100, error: 'stale_runtime' },
    { method: 'session/attach', resultType: 'attached', sessionId: 'session-a', nodeId: 'node-a', target, start: 20, end: 40 },
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

  it.each(['session/fork', 'session/branch', 'session/switchNode', 'turn/start', 'session/attach', 'session/checkpoint', 'session/subscribe'])('never classifies a failed %s as a superseded preview', method => {
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
});
