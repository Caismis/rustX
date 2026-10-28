import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientProjectionHeadView } from '../../../../protocol/app-server/v62';
import type { AppServerClient } from '../../client/app-server';
import { sameValue, useClientSelector } from '../../client/selectors';

/** Read immutable checkpoint metadata from the native projection directory.
 * Already displayed markers may outlive their observation in the same reading
 * presentation, but do not retain read authority and are replaced on refresh. */
export function useCompactionHistory(client: AppServerClient, sessionId: string | undefined, enabled: boolean, retained?: { owner: symbol; keep: boolean }) {
  const facts = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return { observation: view?.attachmentObservation,
      current: enabled && state.connection === 'connected' && view?.attachment === 'attached',
      receipt: view?.compactionRequest?.receipt,
      head: view?.snapshot?.context?.projection_head };
  }, sameValue);
  const [loaded, setLoaded] = useState<{ observation: typeof facts.observation; records: RuntimeClientProjectionHeadView[] }>();
  const presentation = useRef<{ owner: symbol; records: { head: RuntimeClientProjectionHeadView; command: boolean }[] }>(undefined);
  if (!retained?.keep || presentation.current?.owner !== retained.owner) presentation.current = undefined;
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
  if (!facts.current) return retained?.keep ? presentation.current?.records ?? [] : [];
  const complete = loaded?.observation === facts.observation;
  const records = new Map((complete ? loaded?.records ?? [] : presentation.current?.records.map(item => item.head) ?? []).map(head => [head.activation_id, head]));
  if (!facts.head) records.clear();
  if (facts.head) records.set(facts.head.activation_id, facts.head);
  const items = [...records.values()].map(head => ({ head, command: facts.receipt?.checkpoint_id === head.checkpoint_id || !!presentation.current?.records.find(item => item.head.activation_id === head.activation_id && item.command) }));
  if (retained?.keep) presentation.current = { owner: retained.owner, records: items };
  return items;
}
