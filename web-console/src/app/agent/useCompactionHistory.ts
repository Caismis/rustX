import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientProjectionHeadView } from '../../../../protocol/app-server/v82';
import type { AppServerClient } from '../../client/app-server';
import { sameValue, useClientSelector } from '../../client/selectors';

const PAGE = 16n;
type Marker = { head: RuntimeClientProjectionHeadView; command: boolean };

/** One explicit native metadata page plus the active head. Retained display
 * data never retains permission to perform a read after its owner changes. */
export function useCompactionHistory(client: AppServerClient, sessionId: string | undefined, enabled: boolean, retained?: { owner: symbol; keep: boolean }) {
  const facts = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return { observation: view?.attachmentObservation,
      current: enabled && state.connection === 'connected' && view?.attachment === 'attached',
      authority: state.authorityId, target: view?.target,
      receipt: view?.compactionRequest?.receipt,
      head: view?.snapshot?.context?.projection_head };
  }, sameValue);
  const scope = JSON.stringify([facts.authority, sessionId, facts.target?.conversation_id, facts.target?.runtime_incarnation]);
  const [navigation, navigate] = useState<{ scope: string; after: string }>();
  const latestStart = facts.head && BigInt(facts.head.activation_id) > PAGE ? (BigInt(facts.head.activation_id) - PAGE).toString() : '0';
  const after = navigation?.scope === scope ? navigation.after : latestStart;
  const [retry, setRetry] = useState(0);
  const activation = facts.head?.activation_id;
  const [loaded, setLoaded] = useState<{ scope: string; after: string; observation: typeof facts.observation; activation: typeof activation; retry: number; records: RuntimeClientProjectionHeadView[]; error?: string }>();
  const latest = useRef({ scope, after, observation: facts.observation, activation, retry }); latest.current = { scope, after, observation: facts.observation, activation, retry };
  const presentation = useRef<{ owner: symbol; scope: string; records: Marker[] }>(undefined);
  if (!retained?.keep || presentation.current?.owner !== retained.owner) presentation.current = undefined;
  useEffect(() => {
    if (!facts.current || !sessionId || !facts.head) return;
    let mounted = true;
    const current = () => mounted && latest.current.scope === scope && latest.current.after === after
      && latest.current.activation === activation && latest.current.retry === retry
      && latest.current.observation === facts.observation && client.isAttachmentObservationCurrent(sessionId, facts.observation);
    const read = { scope, after, observation: facts.observation, activation, retry };
    void client.projections(sessionId, after).then(page => {
      if (!current()) return;
      if (page.records.length > 16 || page.records.some((head, index) => BigInt(head.activation_id) <= BigInt(after)
        || index > 0 && BigInt(head.activation_id) <= BigInt(page.records[index - 1]!.activation_id))) throw Error('Projection history page did not advance.');
      setLoaded({ ...read, records: page.records });
    }).catch(error => {
      if (current()) setLoaded(previous => ({ ...read,
        records: previous?.scope === scope ? previous.records : [], error: String(error).slice(0, 512) }));
    });
    return () => { mounted = false; };
  }, [client, sessionId, scope, after, facts.observation, facts.current, activation, retry]);
  const page = loaded?.scope === scope ? loaded : undefined;
  const resident = presentation.current?.scope === scope ? presentation.current : undefined;
  const displayed = facts.current ? page?.records ?? resident?.records.map(item => item.head) ?? [] : [];
  const records = new Map(displayed.map(head => [head.activation_id, head]));
  if (facts.current && !facts.head) records.clear();
  if (facts.current && facts.head) records.set(facts.head.activation_id, facts.head);
  const markers = facts.current ? [...records.values()].map(head => ({ head,
    command: facts.receipt?.checkpoint_id === head.checkpoint_id || !!resident?.records.find(item => item.head.activation_id === head.activation_id && item.command) }))
    : retained?.keep ? presentation.current?.records ?? [] : [];
  if (retained?.keep && facts.current) presentation.current = { owner: retained.owner, scope, records: markers };
  const first = page?.records[0], last = page?.records.at(-1);
  const currentPage = facts.current && page?.after === after && page.observation === facts.observation
    && page.activation === activation && page.retry === retry ? page : undefined;
  const busy = facts.current && !!facts.head && !currentPage;
  return {
    markers, busy, error: currentPage?.error,
    earlier: !busy && first && BigInt(first.activation_id) > 1n ? () => navigate({ scope, after: (BigInt(first.activation_id) > PAGE + 1n ? BigInt(first.activation_id) - PAGE - 1n : 0n).toString() }) : undefined,
    later: !busy && last && facts.head && BigInt(last.activation_id) < BigInt(facts.head.activation_id) ? () => navigate({ scope, after: last.activation_id }) : undefined,
    latest: () => { if (!busy) navigate(undefined); },
    retry: () => { if (currentPage?.error) setRetry(retry + 1); },
  };
}
