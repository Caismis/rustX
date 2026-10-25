import type { ContentBinding, ObservationReadCut, RuntimeClientEvent, RuntimeClientSnapshot } from '../../../protocol/app-server/v57';
import { transitionFacts } from '../../../protocol/app-server/projection';

/** Native released-output headers, alongside canonical message heads while
 * the independently cut transcript window catches up. Never live-byte caches. */
export type PublicationHead = Pick<Extract<RuntimeClientEvent, { type: 'assistant_publication_settled' }>, 'audit' | 'transcript_cursor'> & { readonly read_cut: ObservationReadCut; readonly content: ContentBinding[] };

export function publicationHeads(previous: readonly PublicationHead[] | undefined, event: RuntimeClientEvent, snapshot: RuntimeClientSnapshot): readonly PublicationHead[] | undefined {
  const current = previous ?? [];
  const heads = new Map(current.map(head => [head.transcript_cursor, head]));
  for (const fact of transitionFacts(event)) {
    if ((fact.type === 'assistant_publication_settled' || fact.type === 'assistant_publication_deferred_settled') && fact.audit) {
      const record = fact.settlement_head.find(record => record.transcript_cursor === fact.transcript_cursor && record.settlement.stream_id === fact.audit!.stream_id);
      if (record) heads.set(fact.transcript_cursor, { audit: fact.audit, transcript_cursor: fact.transcript_cursor, read_cut: record.read_cut, content: fact.type === 'assistant_publication_deferred_settled' ? fact.content : [] });
    }
  }
  // The native bounded settlement head owns retention. A loaded durable row
  // replaces its event header; no client creates a transcript cut or settlement.
  const retained = [...heads.values()].filter(head => head.read_cut.read.conversation_id === snapshot.conversation_id
    && snapshot.publication_settlements?.some(record => record.transcript_cursor === head.transcript_cursor
      && record.settlement.stream_id === head.audit.stream_id)
    && !snapshot.transcript.entries?.some(entry => entry.cursor === head.transcript_cursor));
  return retained.length === current.length && retained.every((head,index) => head === current[index]) ? previous : retained;
}
