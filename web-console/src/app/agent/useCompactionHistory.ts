import { useEffect, useState } from 'react';
import type { RuntimeClientProjectionHeadView } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import { sameValue, useClientSelector } from '../../client/selectors';

/** Read immutable checkpoint metadata, scoped to the exact attached observation.
 * The current native head renders immediately; older markers come from the
 * projection directory, never from a browser-maintained event history. */
export function useCompactionHistory(client: AppServerClient, sessionId: string | undefined, enabled: boolean) {
  const facts = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return { observation: view?.attachmentObservation,
      current: enabled && state.connection === 'connected' && view?.attachment === 'attached',
      receipt: view?.compactionRequest?.receipt,
      head: view?.snapshot?.context?.projection_head };
  }, sameValue);
  const [loaded, setLoaded] = useState<{ observation: typeof facts.observation; records: RuntimeClientProjectionHeadView[] }>();
  useEffect(() => {
    if (!facts.current || !sessionId || !facts.head) return;
    let mounted = true;
    const current = () => mounted && client.isAttachmentObservationCurrent(sessionId, facts.observation);
    void (async () => {
      const records: RuntimeClientProjectionHeadView[] = [];
      let after = '0';
      do {
        const page = await client.projections(sessionId, after);
        if (!current()) return;
        records.push(...page.records);
        if (page.next_activation == null) break;
        after = page.next_activation;
      } while (current());
      if (current()) setLoaded({ observation: facts.observation, records });
    })().catch(() => {});
    return () => { mounted = false; };
  }, [client, sessionId, facts.observation, facts.current, facts.head?.activation_id]);
  if (!facts.current) return [];
  const records = new Map((loaded?.observation === facts.observation ? loaded?.records ?? [] : []).map(head => [head.activation_id, head]));
  if (facts.head) records.set(facts.head.activation_id, facts.head);
  return [...records.values()].map(head => ({ head, command: facts.receipt?.checkpoint_id === head.checkpoint_id }));
}
