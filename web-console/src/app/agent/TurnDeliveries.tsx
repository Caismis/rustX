import { useEffect, useRef, useState } from 'react';
import type { CheckpointReadCut, RuntimeClientDeliveryHead } from '../../../../protocol/app-server/v59';
import { deliveryHead, deliveryItemKey, deliveryPage, readDeliveryMetadata } from '../../../../protocol/app-server/delivery-observation';
import { latestTurnDeliveries, type DeliveryBatch } from '../../bindings/present';
import { observationSourceIdentity, readObservationValue, useObservationSource } from './ObservationSource';
import { DeliveryCards } from '../components/Artifact';
import { Button } from '../../presentation/primitives/Button';
import { useTranslation } from '../../locale/react';

/** One settled Turn's file row, between its final content and its actions. */
export function TurnDeliveries({ batches, conversationId }: { batches: readonly DeliveryBatch[]; conversationId: string }) {
  try { for (const batch of batches) if (batch.head) deliveryHead(batch.head); }
  catch (error) { return <p role="alert">{String(error).slice(0, 512)}</p>; }
  return <NativeTurnDeliveries batches={batches} conversationId={conversationId}/>;
}
// Loaded pages may grow beyond the wire's eight-item head, within the selected Turn.
type LoadedBatch = { position: string; messageIdentity?: string | null } & ({ deliveries: NonNullable<DeliveryBatch['deliveries']>; head?: never }
  | { head: Omit<RuntimeClientDeliveryHead, 'items'> & { items: RuntimeClientDeliveryHead['items'][number][] }; deliveries?: never });
function NativeTurnDeliveries({ batches, conversationId }: { batches: readonly DeliveryBatch[]; conversationId: string }) {
  const source = useObservationSource(), tx = useTranslation();
  const identity = JSON.stringify([observationSourceIdentity(source), source && 'historyCut' in source ? source.historyCut : undefined, batches, conversationId]);
  const currentIdentity = useRef(identity); currentIdentity.current = identity;
  const generation = useRef(0); useEffect(() => () => { generation.current++; }, [identity]);
  const [state, setState] = useState<{ identity: string; batches: LoadedBatch[]; cut?: CheckpointReadCut; pending?: boolean; error?: string }>();
  const loaded = state?.identity === identity ? state : undefined, shown: readonly LoadedBatch[] = loaded?.batches ?? batches;
  const sourceCid = source && 'target' in source ? source.cut()?.conversation_id : source && 'historyCut' in source ? source.historyCut?.read.conversation_id : source?.cut()?.conversation_id;
  const page = async (head: RuntimeClientDeliveryHead, offset: number, cut: CheckpointReadCut | undefined, current: () => boolean) => {
    if (!source) throw new Error('Native delivery source is unavailable.');
    const captured = cut ?? source.cut();
    if (!captured && ('target' in source || !source.historyCut)) throw new Error('Native delivery source cut is unavailable.');
    const result = 'target' in source
      ? await source.client.request({ method: 'conversation/checkpointPage', params: { target: source.target, agent_id: source.agentId ?? null, cut: captured!, domain: head.domain, offset, limit: 8 } }, 'checkpoint_page', undefined, current)
      : source.historyCut
        ? await source.client.request({ method: 'session/historyPage', params: { session_id: source.sessionId, node_id: source.nodeId ?? null, cut: source.historyCut, domain: head.domain, offset, limit: 8 } }, 'checkpoint_page', undefined, current)
        : await source.client.request({ method: 'session/tracePage', params: { session_id: source.sessionId, node_id: source.nodeId ?? null, cut: captured!, domain: head.domain, offset, limit: 8 } }, 'checkpoint_page', undefined, current);
    if (!current()) throw new Error('Native delivery view changed.');
    const actual = result.page.read_cut;
    if (actual.conversation_id !== sourceCid || cut && JSON.stringify(actual) !== JSON.stringify(cut)
      || 'historyCut' in source && source.historyCut && JSON.stringify(actual.source_cut) !== JSON.stringify(source.historyCut)) throw new Error('Native delivery source cut changed.');
    return { head: deliveryPage(head, result.page, actual, offset), cut: actual };
  };
  const continuation = shown.find(batch => batch.head?.next_offset != null);
  const next = async () => {
    const token = ++generation.current, current = () => token === generation.current && currentIdentity.current === identity && !!source?.owned();
    if (!current() || !continuation?.head || continuation.head.next_offset == null) return;
    setState({ identity, batches: [...shown], cut: loaded?.cut, pending: true });
    try {
      const original = batches.find(batch => batch.position === continuation.position)!.head!;
      const result = await page(original, continuation.head.next_offset, loaded?.cut ?? source?.cut(), current);
      if (current()) setState({ identity, cut: result.cut, batches: shown.map(batch => batch !== continuation ? batch
        : { position: batch.position, messageIdentity: batch.messageIdentity, head: { ...result.head, items: [...continuation.head!.items, ...result.head.items] } }) });
    } catch (error) { if (current()) setState({ identity, batches: [...shown], cut: loaded?.cut, error: String(error).slice(0, 512) }); }
  };
  const missing = shown.flatMap(batch => batch.head?.items.filter(item => item.file == null) ?? []), missingKey = JSON.stringify(missing), active = !!source?.owned();
  useEffect(() => {
    if (!source || !active || !missing.length) return;
    let mounted = true; const token = ++generation.current, current = () => mounted && token === generation.current && currentIdentity.current === identity && source.owned();
    setState({ identity, batches: [...shown], cut: loaded?.cut, pending: true });
    void (async () => {
      let cut = loaded?.cut ?? source.cut();
      if (!cut) cut = (await page(batches.find(batch => batch.head)!.head!, 0, undefined, current)).cut;
      const files = new Map<string, Awaited<ReturnType<typeof readDeliveryMetadata>>>();
      for (const item of missing) {
        const file = await readDeliveryMetadata(item, cut, async offset => {
          if ('historyCut' in source && source.historyCut) return (await source.client.request({ method: 'session/historyValueRead', params: { session_id: source.sessionId, node_id: source.nodeId ?? null, cut: source.historyCut, domain: item.metadata_domain, item_offset: 0, path: [], offset, max_bytes: 65_536 } }, 'checkpoint_value_chunk', undefined, current)).chunk;
          return (await readObservationValue(source, cut!, item.metadata_domain, [], offset, current)).chunk;
        }, current);
        if (!current()) return; files.set(deliveryItemKey(item), file);
      }
      if (current()) setState({ identity, cut, batches: shown.map(batch => !batch.head ? batch : { position: batch.position, messageIdentity: batch.messageIdentity, head: { ...batch.head,
        items: batch.head.items.map(item => files.has(deliveryItemKey(item)) ? { ...item, file: files.get(deliveryItemKey(item)), metadata_deferred: false } : item) } }) });
    })().catch(error => { if (current()) setState({ identity, batches: [...shown], cut: loaded?.cut, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [identity, missingKey, source?.client, active]);
  return <section data-turn-deliveries>
    <DeliveryCards conversationId={sourceCid ?? conversationId} owned={source?.owned} deliveries={latestTurnDeliveries(shown)}/>
    {continuation && <Button disabled={loaded?.pending || !source?.owned()} onClick={() => void next()}>{tx('common:activity.show-more')}</Button>}
    {loaded?.error && <p role="alert">{loaded.error}</p>}
  </section>;
}
