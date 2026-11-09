import { useContext, useEffect, useState } from 'react';
import { consumerAddress } from '../../../document/analysis';
import { encodeField, type FieldAddress } from '../../../document/fields';
import { targetKey } from '../../../port';
import { DraftOwner } from './drafts';
import { useCandidateAnalysis } from './analysis';

/** Confirmed presentation stays stable while the same reference is reanalyzed.
 * A control can keep its presentation identity when an edit moves the
 * reference between operands. The analysis query always uses its actual field. */
export function useReferenceSchema(
  address: FieldAddress,
  path: readonly string[],
  presentationAddress = address,
) {
  const owner = useContext(DraftOwner)!,
    context = owner.editor.getSnapshot().context;
  const consumer = context.document && consumerAddress(context.document, address);
  const result = useCandidateAnalysis(
    consumer ? { consumer, reference: [...path], limit: 1 } : undefined,
    !!path.length && !!consumer,
  );
  const fresh = result?.report?.analysis?.selected?.schema as Record<string, unknown> | undefined;
  const identity = JSON.stringify([targetKey(context.target), encodeField(presentationAddress), path]);
  const [confirmed, setConfirmed] = useState<{ identity: string; schema: Record<string, unknown> }>();
  useEffect(() => {
    if (fresh) setConfirmed({ identity, schema: fresh });
  }, [fresh, identity]);
  return fresh ?? (confirmed?.identity === identity ? confirmed.schema : undefined);
}
