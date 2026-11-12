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
  /** Page-local WebSocket identity; a reconnect starts a new authority. */
  connection?: number;
  threadId?: string;
  resultThreadId?: string;
  /** Opaque JSON encoding of the exact native attachment identity tuple. */
  target?: string;
}

/** Narrow optional node identities before writing a string-keyed lineage map. */
export function nodeLineageIdentity(
  sessionId: string | undefined,
  node: { id?: string; conversation_id?: string } | undefined,
): { conversationId: string; sessionId: string; nodeId: string } | undefined {
  if (typeof sessionId !== 'string' || !sessionId || !node) return undefined;
  if (typeof node.id !== 'string' || !node.id) return undefined;
  if (typeof node.conversation_id !== 'string' || !node.conversation_id) return undefined;
  return { conversationId: node.conversation_id, sessionId, nodeId: node.id };
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
  const following = successful.filter(attach => attach.method === 'thread/resume' && attach.resultType === 'attached' && attach.target
    && successful.some(checkpoint => checkpoint.method === 'session/checkpoint' && checkpoint.resultType === 'checkpoint'
      && checkpoint.target === attach.target && attach.end <= checkpoint.start
      && successful.some(subscribe => subscribe.method === 'session/subscribe' && subscribe.resultType === 'subscribed'
        && subscribe.target === attach.target && checkpoint.end <= subscribe.start)));
  return rpc.filter(history => history.method === 'session/history' && history.historyAt === 'latest' && history.error === 'stale_runtime'
    && history.sessionId && history.nodeId && completed(history)
    && following.some(attach => attach.sessionId === history.sessionId && attach.nodeId === history.nodeId
      && history.start < attach.end && attach.start < history.end));
}

/** Strong status reads can fail while native status capture is busy. Require
 * a new successful strong read of that exact Thread on the same connection;
 * unrelated reads, earlier/overlapping reads and reconnects prove no recovery. */
export function recoveredThreadStatusReads(rpc: readonly RequestTiming[]): RequestTiming[] {
  const recovered = rpc.filter((row): row is CompletedTiming => completed(row)
    && row.method === 'thread/read' && row.error === undefined && row.resultType === 'thread'
    && !!row.threadId && row.resultThreadId === row.threadId);
  return rpc.filter(row => completed(row) && row.method === 'thread/read' && row.error === 'observation_unavailable'
    && row.connection !== undefined && Number.isSafeInteger(row.connection) && row.connection > 0 && !!row.threadId
    && recovered.some(read => read.connection === row.connection && read.threadId === row.threadId && read.start >= row.end));
}
