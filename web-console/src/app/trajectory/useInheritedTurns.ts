import { useEffect, useState } from 'react';
import type { ObservedTurn } from '../../../../protocol/app-server/v80';
import type { AppServerClient } from '../../client/app-server';
import { TURN_PAGE_SIZE } from '../../client/transcript';

export interface TurnSource {
  client: AppServerClient;
  session: string;
  conversation: string;
}

// The keyed trajectory owns this reader. Earlier turns have their original
// conversation identity and no event IDs in the selected branch's trace.
export function useInheritedTurns(source?: TurnSource) {
  const [offset, setOffset] = useState(0);
  const [result, setResult] = useState<{ turns: ObservedTurn[]; next?: number }>();
  const [attempt, setAttempt] = useState(0);
  const [retry, setRetry] = useState(false);
  const client = source?.client, session = source?.session, conversation = source?.conversation;
  useEffect(() => {
    if (!client || !session || !conversation) return;
    let current = true;
    setRetry(false);
    const read = async () => {
      const page = await client.previewTurns(session, offset, () => current);
      if (!current) return;
      if (!page || page.cut.conversation_id !== conversation || page.offset !== offset) {
        setRetry(true);
        return;
      }
      const inherited = page.turns.filter(turn => turn.id.conversation_id !== conversation);
      const after = offset + page.turns.length;
      const hasMore = inherited.length === TURN_PAGE_SIZE && BigInt(after) < BigInt(page.total);
      setResult(previous => ({
        turns: [...(offset ? previous?.turns.slice(0, offset) ?? [] : []), ...inherited],
        next: hasMore ? after : undefined,
      }));
    };
    void read().catch(() => { if (current) setRetry(true); });
    return () => { current = false; };
  }, [client, session, conversation, offset, attempt]);
  return {
    turns: source ? result?.turns : undefined,
    more: !source ? undefined : retry ? () => setAttempt(n => n + 1)
      : result?.next !== undefined && offset !== result.next ? () => setOffset(result.next!) : undefined,
  };
}
