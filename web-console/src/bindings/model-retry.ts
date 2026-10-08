import type { TraceRecord } from '../../../protocol/app-server/v38';

export interface ModelRetryNotice {
  key: string;
  request: TraceRecord;
  failure?: TraceRecord;
  retry: number;
}

/** Read existing native request facts only. A failed request does not prove
 * that a retry was scheduled; only a new request proves that it started. */
export function modelRetryNotices(records: readonly TraceRecord[], attemptId: string): ModelRetryNotice[] {
  const steps = new Map<string, TraceRecord[]>();
  for (const record of records) {
    if (record.kind !== 'request' || !record.request || record.location.attempt_id !== attemptId || !record.location.step_id) continue;
    const list = steps.get(record.location.step_id) ?? [];
    list.push(record); steps.set(record.location.step_id, list);
  }
  return [...steps].flatMap(([step, requests]) => {
    const request = requests.at(-1)!;
    const facts = request.request!;
    const predecessor = facts.predecessor;
    const failure = facts.failure_kind === 'timeout' ? request
      : facts.previous_failure_kind === 'timeout' && predecessor.availability === 'available'
        ? requests.find(row => row.request?.request_id === predecessor.request_id && row.request?.failure_kind === 'timeout') : undefined;
    if (!failure && facts.previous_failure_kind !== 'timeout') return [];
    return [{ key: JSON.stringify([attemptId, step]), request, failure, retry: facts.retry_number }];
  });
}
