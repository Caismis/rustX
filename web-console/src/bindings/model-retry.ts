import type { TraceRecord } from '../../../protocol/app-server/v42';

export interface ModelRetryNotice {
  key: string;
  attemptId: string;
  request: TraceRecord;
  failure?: TraceRecord;
  retry: number;
  /** Native request publications in retry order, excluding the initial failure. */
  messageIds: readonly string[];
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
    const firstRetry = requests.findIndex(row => row.request!.previous_failure_kind === 'timeout');
    return [{ key: JSON.stringify([attemptId, step]), attemptId, request, failure, retry: facts.retry_number,
      messageIds: firstRetry < 0 ? [] : requests.slice(firstRetry).map(row => row.request!.assistant_message_id) }];
  });
}

/** Retry rows precede their first retained native publication. A request with
 * no publication retains its Attempt feedback seat; adjacency proves nothing. */
export function modelRetryPlacements(records: readonly TraceRecord[], messageIds: ReadonlySet<string>) {
  const attempts = new Set(records.flatMap(record => record.kind === 'request' && record.location.attempt_id ? [record.location.attempt_id] : []));
  return [...attempts].flatMap(attempt => modelRetryNotices(records, attempt).map(notice => ({
    notice, beforeMessageId: notice.messageIds.find(id => messageIds.has(id)),
  })));
}
