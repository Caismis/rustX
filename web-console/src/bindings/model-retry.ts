import {traceAttemptKey,traceStepKey,traceFieldKey} from '../../../protocol/app-server/trace-identity';
import type { TraceLocation, TraceRecord } from '../../../protocol/app-server/v57';

export interface ModelRetryNotice {
  key: string;
  ownerKey: string;
  owner: Pick<TraceLocation,'attempt_id'|'attempt_id_source'>;
  request: TraceRecord;
  failure?: TraceRecord;
  retry: number;
  /** Native request publications in retry order, excluding the initial failure. */
  messageIds: readonly string[];
}

/** Read existing native request facts only. A failed request does not prove
 * that a retry was scheduled; only a new request proves that it started. */
export function modelRetryNotices(records: readonly TraceRecord[], owner: ModelRetryNotice['owner']): ModelRetryNotice[] {
  const ownerKey=traceAttemptKey(owner);
  if(!ownerKey)return [];
  const steps = new Map<string, TraceRecord[]>();
  for (const record of records) {
    const step=traceStepKey(record.location);
    if (record.kind !== 'request' || !record.request || traceAttemptKey(record.location) !== ownerKey || !step) continue;
    const list = steps.get(step) ?? [];
    list.push(record); steps.set(step, list);
  }
  return [...steps].flatMap(([step, requests]) => {
    const request = requests.at(-1)!;
    const facts = request.request!;
    const predecessor = facts.predecessor;
    const failure = facts.failure_kind === 'timeout' ? request
      : facts.previous_failure_kind === 'timeout' && predecessor.availability === 'available'
        ? requests.find(row => row.request?.failure_kind === 'timeout' && (predecessor.request_id!=null ? row.request.request_id===predecessor.request_id : predecessor.request_id_source!=null && row.request.request_id_source!=null && traceFieldKey(row.request.request_id_source)===traceFieldKey(predecessor.request_id_source))) : undefined;
    if (!failure && facts.previous_failure_kind !== 'timeout') return [];
    const firstRetry = requests.findIndex(row => row.request!.previous_failure_kind === 'timeout');
    return [{ key: JSON.stringify([ownerKey, step]), ownerKey,owner, request, failure, retry: facts.retry_number,
      messageIds: firstRetry < 0 ? [] : requests.slice(firstRetry).flatMap(row => row.request!.assistant_message_id!=null?[row.request!.assistant_message_id]:[]) }];
  });
}

/** Retry rows precede their first retained native publication. A request with
 * no publication retains its Attempt feedback seat; adjacency proves nothing. */
export function modelRetryPlacements(records: readonly TraceRecord[], messageIds: ReadonlySet<string>) {
  const attempts = new Map<string,ModelRetryNotice['owner']>();
  for(const record of records){const key=traceAttemptKey(record.location);if(record.kind==='request'&&key)attempts.set(key,record.location);}
  return [...attempts.values()].flatMap(attempt => modelRetryNotices(records, attempt).map(notice => ({
    notice, beforeMessageId: notice.messageIds.find(id => messageIds.has(id)),
  })));
}
