import { useEffect, useRef, useState } from 'react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v57';
import { useTranslation } from '../../locale/react';
import { observationSourceIdentity, useObservationSource } from './ObservationSource';
import { readToolOccurrence } from './tool-source';
import { Tool } from './Tool';

/** A canonical call is complete; a deferred execution header is not assembly. */
export function CanonicalTool({ messageId, blockIndex, name }: { messageId: string; blockIndex: number; name: string }) {
  const tx = useTranslation(), source = useObservationSource();
  const frozen = !!source?.historyFrozen || !!(source && 'historyCut' in source && source.historyCut);
  const identity = JSON.stringify([observationSourceIdentity(source), frozen && (source?.history?.source_cut ?? (source && 'historyCut' in source ? source.historyCut : undefined)), messageId, blockIndex]);
  const readKey = JSON.stringify([identity, frozen ? undefined : source?.cut()?.revision]);
  const latest = useRef({ identity, readKey, source }); latest.current = { identity, readKey, source };
  const [retry, setRetry] = useState(0), [state, setState] = useState<{ identity: string; tool?: ForegroundToolExecution; error?: string }>();
  const loaded = state?.identity === identity ? state : undefined;
  useEffect(() => {
    if (!source?.owned()) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.readKey === readKey && !!latest.current.source?.owned();
    void readToolOccurrence(source, { type: 'by_message_id', message_id: messageId }, blockIndex, current, frozen)
      .then(({ tool }) => { if (current()) setState({ identity, tool }); })
      .catch(error => { if (current()) setState({ identity, error: String(error) }); });
    return () => { mounted = false; };
  }, [readKey, retry, source?.client]);
  return loaded?.tool ? <Tool tool={loaded.tool}/>
    : loaded?.error || !source ? <p role="alert">{loaded?.error ?? tx('agent:message.tool-unavailable', { name })} {source && <button type="button" onClick={() => { setState(undefined); setRetry(value => value + 1); }}>{tx('agent:message.retry-tool')}</button>}</p>
      : <p role="status">{tx('common:activity.loading')}</p>;
}
