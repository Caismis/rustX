import { useEffect, useRef, useState } from 'react';
import type { CheckpointReadCut, ProjectionDetail, ProjectionPage } from '../../../../protocol/app-server/v52';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { TraceFields } from '../trajectory/TraceFields';
import type { TraceFieldReader } from '../../client/trace';
import { ProjectionSummary } from './ContextSeat';

type Props = { client: AppServerClient; sessionId: string };

/** Historical inspection has Session-owned pages, independent of execution rows. */
export function ProjectionHistory(props: Props) {
  return <ProjectionHistoryPages key={props.sessionId} {...props}/>;
}

function ProjectionHistoryPages({ client, sessionId }: Props) {
  const tx = useTranslation();
  const [page, setPage] = useState<ProjectionPage>();
  const [detail, setDetail] = useState<ProjectionDetail & { sources: Map<number, {cut:CheckpointReadCut;reader?:TraceFieldReader}> }>();
  const [historyError, setHistoryError] = useState<string>();
  const [detailError, setDetailError] = useState<string>();
  const [historyLoading, setHistoryLoading] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const historyRequest = useRef<symbol | null>(null);
  const detailRequest = useRef<symbol | null>(null);
  const selected = useRef<string | undefined>(undefined);
  useEffect(() => () => { historyRequest.current = null; detailRequest.current = null; }, []);

  async function load(after = '0') {
    if (historyRequest.current) return;
    const request = Symbol(); historyRequest.current = request;
    setHistoryLoading(true); setHistoryError(undefined);
    try {
      const next = await client.projections(sessionId, after);
      if (historyRequest.current !== request) return;
      setPage(previous => {
        const records = [...(previous?.records ?? [])];
        const seen = new Set(records.map(record => record.activation_id));
        for (const record of next.records) if (!seen.has(record.activation_id)) {
          seen.add(record.activation_id); records.push(record);
        }
        records.sort((a, b) => BigInt(a.activation_id) < BigInt(b.activation_id) ? -1 : BigInt(a.activation_id) > BigInt(b.activation_id) ? 1 : 0);
        return { records, next_activation: next.next_activation };
      });
    } catch (failure) {
      if (historyRequest.current === request) setHistoryError(String(failure));
    } finally {
      if (historyRequest.current === request) { historyRequest.current = null; setHistoryLoading(false); }
    }
  }
  async function inspect(checkpoint: string, offset = 0) {
    if (offset !== 0 && (selected.current !== checkpoint || detailRequest.current)) return;
    if (offset === 0 && selected.current === checkpoint && (detail || detailRequest.current)) return;
    if (selected.current !== checkpoint) { selected.current = checkpoint; setDetail(undefined); }
    const request = Symbol(); detailRequest.current = request;
    setDetailLoading(true); setDetailError(undefined);
    try {
      const next = await client.projectionDetail(sessionId, checkpoint, offset);
      if (detailRequest.current !== request || selected.current !== checkpoint) return;
      if (next.checkpoint.checkpoint_id !== checkpoint) throw new Error('Projection detail returned another checkpoint');
      if (next.items.some(item => item.occurrence.checkpoint_id !== checkpoint)) throw new Error('Projection item returned another checkpoint');
      setDetail(previous => {
        const items = offset === 0 ? [] : [...(previous?.items ?? [])];
        const sources = offset === 0 ? new Map<number, {cut:CheckpointReadCut;reader?:TraceFieldReader}>() : new Map(previous?.sources);
        const seen = new Set(items.map(item => item.occurrence.item_index));
        for (const item of next.items) {
          if (seen.has(item.occurrence.item_index)) continue;
          seen.add(item.occurrence.item_index); items.push(item);
          sources.set(item.occurrence.item_index, {cut:next.read_cut,reader:client.projectionDetailReader(next)});
        }
        items.sort((a, b) => a.occurrence.item_index - b.occurrence.item_index);
        return { ...next, checkpoint: offset === 0 ? next.checkpoint : previous?.checkpoint ?? next.checkpoint, items, sources };
      });
    } catch (failure) {
      if (detailRequest.current === request) setDetailError(String(failure));
    } finally {
      if (detailRequest.current === request) { detailRequest.current = null; setDetailLoading(false); }
    }
  }
  return <details data-projection-history="" aria-busy={historyLoading || detailLoading} onToggle={event => { if (event.currentTarget.open && !page) void load(); }}>
    <summary>{tx('agent:context.projection-history')}</summary>
    {historyError && <p role="alert">{historyError}</p>}
    {page?.records.map(head => <section key={head.activation_id}>
      <ProjectionSummary head={head}/>
      <button onClick={() => { void inspect(head.checkpoint_id); }}>{tx('agent:context.projection-inspect')}</button>
    </section>)}
    {page?.next_activation && <button disabled={historyLoading} onClick={() => { void load(page.next_activation!); }}>{tx('agent:context.projection-more')}</button>}
    {detailError && <p role="alert">{detailError}</p>}
    {detail && <section data-projection-detail={detail.checkpoint.checkpoint_id} aria-busy={detailLoading}>
      {detail.items.map(item => <article key={item.occurrence.item_index}>
        <p>{item.occurrence.item_index + 1} · {item.content.role} · {item.content.provenance.type}</p>
        {item.content.blocks.map((block, index) => block.type === 'text'
          ? <MarkdownText key={index} text={block.text.text}/>
          : <pre key={index}>{JSON.stringify(block, null, 2)}</pre>)}
        {item.content.provenance_source && <TraceFields fields={[item.content.provenance_source]} cut={detail.sources.get(item.occurrence.item_index)?.cut} reader={detail.sources.get(item.occurrence.item_index)?.reader} owned={()=>selected.current===detail.checkpoint.checkpoint_id}/>}
        {item.content.truncated && <p>{tx('agent:context.projection-truncated')}</p>}
      </article>)}
      {detail.next_item != null && <button disabled={detailLoading} onClick={() => { void inspect(detail.checkpoint.checkpoint_id, detail.next_item!); }}>{tx('agent:context.projection-more')}</button>}
    </section>}
  </details>;
}
