export interface RequestTiming {
  method: string;
  start: number;
  end?: number;
  bytes?: number;
  error?: string;
  historyAt?: string;
  resultType?: string;
  sessionId?: string;
  nodeId?: string;
  /** Opaque JSON encoding of the exact native attachment identity tuple. */
  target?: string;
}

type CompletedTiming = RequestTiming & { end: number };

function completed(row: RequestTiming): row is CompletedTiming {
  return Number.isFinite(row.start) && row.start >= 0 && row.end !== undefined
    && Number.isFinite(row.end) && row.end >= row.start;
}

/** A cold preview may lose to native runtime preparation. Accept only that
 * exact refusal when the same node subsequently establishes a complete live
 * observation. Return the original evidence rows; never erase their errors. */
export function supersededHistoryReads(rpc: readonly RequestTiming[]): RequestTiming[] {
  const successful = rpc.filter((row): row is CompletedTiming => completed(row) && row.error === undefined);
  const following = successful.filter(attach => attach.method === 'session/attach' && attach.resultType === 'attached' && attach.target
    && successful.some(checkpoint => checkpoint.method === 'session/checkpoint' && checkpoint.resultType === 'checkpoint'
      && checkpoint.target === attach.target && attach.end <= checkpoint.start
      && successful.some(subscribe => subscribe.method === 'session/subscribe' && subscribe.resultType === 'subscribed'
        && subscribe.target === attach.target && checkpoint.end <= subscribe.start)));
  return rpc.filter(history => history.method === 'session/history' && history.historyAt === 'latest' && history.error === 'stale_runtime'
    && history.sessionId && history.nodeId && completed(history)
    && following.some(attach => attach.sessionId === history.sessionId && attach.nodeId === history.nodeId
      && history.start < attach.end && attach.start < history.end));
}
