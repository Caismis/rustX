import { useEffect, useRef, useState } from 'react';
import type { CheckpointReadCut, TraceInspectionCompactionCall, TraceInspectionCompactionCalls, RuntimeClientProjectionHeadView } from '../../../../protocol/app-server/v81';
import { checkpointReadCutKey } from '../../../../protocol/app-server/content-identity';
import { compactionCallsPage, compactionPageOffset, compactionRequestsPage, type CompactionPhysicalRequest } from '../../client/compaction-inspection';
import { traceReaderScope, type TraceFieldReader } from '../../client/trace';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import css from './CompactionInspection.module.css';

interface CallView { identity: string; items: TraceInspectionCompactionCall[]; next?: string | null; busy?: boolean; error?: string }

/** One selected native operation and one frozen read family; all actions read. */
export function CompactionInspection({ initial, checkpoint, cut, reader, accepted }: {
  initial: TraceInspectionCompactionCalls; checkpoint?: RuntimeClientProjectionHeadView | null;
  cut: CheckpointReadCut; reader: TraceFieldReader; accepted?: number;
}) {
  const tx = useTranslation();
  const identity = JSON.stringify([traceReaderScope(reader), checkpointReadCutKey(cut), initial]);
  const owner = JSON.stringify([traceReaderScope(reader), cut.conversation_id, cut.activation_id, initial.domain]);
  const [selection, select] = useState<{ owner: string; call: TraceInspectionCompactionCall }>();
  const [more, setMore] = useState<CallView>();
  const [located, setLocated] = useState<{ identity: string; call?: TraceInspectionCompactionCall; error?: string }>();
  const collection: CallView = more?.identity === identity ? more : { identity, items: initial.items, next: initial.next_offset };
  const selectedId = selection?.owner === owner ? selection.call.call_id : collection.items[0]?.call_id;
  const call = collection.items.find(item => item.call_id === selectedId)
    ?? (located?.identity === identity && located.call?.call_id === selectedId ? located.call : undefined);
  const latest = useRef({ identity, reader, selectedId }); latest.current = { identity, reader, selectedId };
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const [retry, setRetry] = useState(0);

  // A changing cut keeps the selected native Call, including one beyond the
  // first page. Its old location is a selector, never a current metadata body.
  useEffect(() => {
    if (call || !selectedId || selection?.owner !== owner) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.reader === reader && latest.current.selectedId === selectedId;
    const after = (BigInt(selection.call.started_sequence) - 1n).toString();
    void reader.page(cut, initial.domain, compactionPageOffset(after), current).then(page => {
      const selected = compactionCallsPage(page, cut, initial, after).items.find(item => item.call_id === selectedId);
      if (!selected) throw Error('Selected native Compaction Call is unavailable at this cut.');
      if (current()) setLocated({ identity, call: selected });
    }).catch(error => { if (current()) setLocated({ identity, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [identity, reader, selectedId, retry]);

  const loadMore = async () => {
    if (collection.next == null || collection.busy) return;
    const current = () => alive.current && latest.current.identity === identity && latest.current.reader === reader;
    setMore({ ...collection, busy: true, error: undefined });
    try {
      const page = await reader.page(cut, initial.domain, compactionPageOffset(collection.next), current);
      const next = compactionCallsPage(page, cut, initial, collection.next);
      if (next.items.some(item => collection.items.some(previous => previous.call_id === item.call_id))) throw Error('Compaction Call page repeated an identity.');
      const loaded = BigInt(collection.items.length + next.items.length);
      if (loaded > BigInt(initial.total) || next.next_offset == null && loaded !== BigInt(initial.total)) throw Error('Compaction Call page omitted native entries.');
      if (current()) setMore({ identity, items: [...collection.items, ...next.items], next: next.next_offset });
    } catch (error) {
      if (current()) setMore({ ...collection, error: String(error).slice(0, 512) });
    }
  };

  const option = call ?? (selection?.owner === owner ? selection.call : undefined);
  return <section className={css.root} data-compaction-inspection="">
    {checkpoint && <details open className={css.checkpoint} data-inspected-checkpoint={checkpoint.checkpoint_id}>
      <summary>{tx('trajectory:compaction.checkpoint')} · <code>{checkpoint.checkpoint_id}</code></summary>
      {checkpoint.summary_preview ? <>
        <MarkdownText text={checkpoint.summary_preview.text}/>
        {checkpoint.summary_preview.truncated && <p>{tx('agent:context.summary-truncated')}</p>}
      </> : <p>{tx('agent:context.summary-unavailable')}</p>}
    </details>}
    <dl className={css.facts}>
      <div><dt>{tx('trajectory:compaction.calls')}</dt><dd>{BigInt(initial.total).toLocaleString()}</dd></div>
      {accepted != null && <div><dt>{tx('trajectory:compaction.accepted')}</dt><dd>{accepted}</dd></div>}
      <div><dt>{tx('trajectory:compaction.read-cut')}</dt><dd>{cut.through}</dd></div>
    </dl>
    {collection.items.length ? <label className={css.select}>
      <span>{tx('trajectory:compaction.call')}</span>
      <select aria-label={tx('trajectory:compaction.call')} value={selectedId} onChange={event => {
        const selected = collection.items.find(item => item.call_id === event.target.value);
        if (selected) select({ owner, call: selected });
      }}>
        {option && !collection.items.some(item => item.call_id === option.call_id) && <option value={option.call_id}>{option.call_id}</option>}
        {collection.items.map(item => <option key={item.call_id} value={item.call_id}>{item.call_id}</option>)}
      </select>
    </label> : <p>{tx('trajectory:compaction.no-requests')}</p>}
    {collection.next != null && <Button disabled={collection.busy} onClick={() => { void loadMore(); }}>{tx(collection.busy ? 'trajectory:compaction.loading' : 'trajectory:compaction.more-calls')}</Button>}
    {collection.error && <p role="alert">{collection.error}</p>}
    {!call && selectedId && <p role={located?.error ? 'alert' : 'status'}>{located?.identity === identity && located.error || tx('trajectory:compaction.loading')} <Button onClick={() => setRetry(value => value + 1)}>{tx('trajectory:compaction.retry')}</Button></p>}
    {call && <PhysicalRequests call={call} cut={cut} reader={reader} owner={owner}/>}
  </section>;
}

function PhysicalRequests({ call, cut, reader, owner }: { call: TraceInspectionCompactionCall; cut: CheckpointReadCut; reader: TraceFieldReader; owner: string }) {
  const tx = useTranslation();
  const identity = JSON.stringify([checkpointReadCutKey(cut), call]);
  const requestOwner = JSON.stringify([owner, call.call_id]);
  const latest = useRef({ identity, reader }); latest.current = { identity, reader };
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const [retry, setRetry] = useState(0);
  const [state, setState] = useState<{ identity: string; owner: string; items?: CompactionPhysicalRequest[]; next?: string | null; busy?: boolean; error?: string; through: string }>();
  const shown = state?.owner === requestOwner ? state : undefined;
  const current = () => alive.current && latest.current.identity === identity && latest.current.reader === reader;
  const load = async (offset: string, append: boolean, owned: () => boolean) => {
    try {
      const page = await reader.page(cut, call.requests_domain, compactionPageOffset(offset), owned);
      const requests = compactionRequestsPage(page, cut, call, offset);
      if (owned()) setState(previous => ({ identity, owner: requestOwner,
        items: [...(append && previous?.identity === identity ? previous.items ?? [] : []), ...requests.items], next: requests.next, through: cut.through }));
    } catch (error) {
      if (owned()) setState(previous => ({ identity, owner: requestOwner,
        items: previous?.owner === requestOwner ? previous.items : undefined, through: previous?.through ?? cut.through, error: String(error).slice(0, 512) }));
    }
  };
  useEffect(() => {
    let mounted = true;
    void load('0', false, () => mounted && current());
    return () => { mounted = false; };
  }, [identity, reader, retry]);
  const strategy = call.strategy === 'native' ? 'trajectory:compaction.native'
    : call.strategy === 'prompt' ? 'trajectory:compaction.prompt'
      : call.strategy ? 'trajectory:compaction.fallback' : undefined;
  const tokens = (value?: string) => value == null ? tx('trajectory:usage.not-reported') : BigInt(value).toLocaleString();
  return <div className={css.requests}>
    <dl className={css.facts}>
      <div><dt>{tx('trajectory:compaction.strategy')}</dt><dd>{strategy ? tx(strategy) : '—'}</dd></div>
      <div><dt>{tx('trajectory:compaction.preceding')}</dt><dd>{call.preceding_call?.call_id ?? '—'}</dd></div>
      <div><dt>{tx('trajectory:compaction.previous-summary')}</dt><dd>{call.previous_summary?.call_id ?? '—'}</dd></div>
    </dl>
    {(!shown || shown.identity !== identity) && <p role="status">{tx(shown?.items ? 'trajectory:compaction.refreshing' : 'trajectory:compaction.loading')}</p>}
    {shown?.error && <p role="alert">{shown.error} <Button onClick={() => setRetry(value => value + 1)}>{tx('trajectory:compaction.retry')}</Button></p>}
    {shown?.items?.map(request => <div className={css.request} key={request.id} data-compaction-request={request.id}>
      <h4>{tx('trajectory:compaction.physical-request')} {request.ordinal + 1}</h4>
      <dl className={css.facts}>
        <div><dt>{tx('trajectory:compaction.request-id')}</dt><dd><code>{request.id}</code></dd></div>
        <div><dt>{tx('trajectory:compaction.provider')}</dt><dd>{request.provider}</dd></div>
        <div><dt>{tx('trajectory:compaction.model')}</dt><dd>{request.model}</dd></div>
        <div><dt>{tx('trajectory:compaction.profile')}</dt><dd>{request.profile === null ? tx('trajectory:compaction.base-profile') : request.profile ?? '—'}</dd></div>
        <div><dt>{tx('trajectory:compaction.wire-model')}</dt><dd>{request.wireModel}</dd></div>
        <div><dt>{tx('trajectory:details.status')}</dt><dd>{request.terminal === 'completed' ? tx('trajectory:compaction.request-completed') : request.terminal === 'failed' ? tx('trajectory:compaction.request-failed') : tx('trajectory:compaction.unsettled')}</dd></div>
        {request.failure && <div><dt>{tx('trajectory:compaction.failure-kind')}</dt><dd>{request.failure}</dd></div>}
        <div><dt>{tx('trajectory:usage.input')}</dt><dd>{tokens(request.input)}</dd></div>
        <div><dt>{tx('trajectory:usage.output')}</dt><dd>{tokens(request.output)}</dd></div>
        <div><dt>{tx('trajectory:usage.reasoning')}</dt><dd>{tokens(request.reasoning)}</dd></div>
      </dl>
      {request.remoteUncertain && <p>{tx('trajectory:compaction.remote-unknown')}</p>}
    </div>)}
    {shown?.items?.length === 0 && <p>{tx('trajectory:compaction.no-requests')}</p>}
    {shown?.identity === identity && shown.next != null && <Button disabled={shown.busy} onClick={() => {
      setState(previous => previous && { ...previous, busy: true });
      void load(shown.next!, true, current);
    }}>{tx('trajectory:compaction.more-requests')}</Button>}
  </div>;
}
