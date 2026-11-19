import { useEffect, useMemo, useState } from 'react';
import type { ObservedTurn } from '../../../../protocol/app-server/v80';
import type { AppServerClient } from '../../client/app-server';
import { TURN_PAGE_SIZE } from '../../client/transcript';

export interface TurnSource {
  client: AppServerClient;
  session: string;
  conversation: string;
}

// Earlier turns have their original
// conversation identity and no event IDs in the selected branch's trace.
export function useInheritedTurns(source?: TurnSource) {
  const client = source?.client, session = source?.session, conversation = source?.conversation;
  const owner = useMemo(() => ({ client, session, conversation }), [client, session, conversation]);
  const [request, setRequest] = useState({ owner, offset: 0, attempt: 0 });
  const offset = request.owner === owner ? request.offset : 0;
  const [result, setResult] = useState<{ owner: typeof owner; turns: ObservedTurn[]; next?: number }>();
  const [failure, setFailure] = useState<typeof owner>();
  useEffect(() => {
    if (!client || !session || !conversation) return;
    let current = true;
    setFailure(undefined);
    const read = async () => {
      const page = await client.previewTurns(session, offset, () => current);
      if (!current) return;
      if (!page || page.cut.conversation_id !== conversation || page.offset !== offset) {
        setFailure(owner);
        return;
      }
      const inherited = page.turns.filter(turn => turn.id.conversation_id !== conversation);
      const after = offset + page.turns.length;
      const hasMore = inherited.length === TURN_PAGE_SIZE && BigInt(after) < BigInt(page.total);
      setResult(previous => ({
        owner, turns: [...(offset && previous?.owner === owner ? previous.turns.slice(0, offset) : []), ...inherited],
        next: hasMore ? after : undefined,
      }));
    };
    void read().catch(() => { if (current) setFailure(owner); });
    return () => { current = false; };
  }, [owner, client, session, conversation, offset, request.attempt]);
  return {
    ready: !source || result?.owner === owner || failure === owner,
    turns: source && result?.owner === owner ? result.turns : undefined,
    more: !source ? undefined : failure === owner ? () => setRequest({ owner, offset, attempt: request.attempt + 1 })
      : result?.owner === owner && result.next !== undefined && offset !== result.next ? () => setRequest({ owner, offset: result.next!, attempt: 0 }) : undefined,
  };
}
