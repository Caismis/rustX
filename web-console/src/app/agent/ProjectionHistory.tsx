import { useState } from 'react';
import type { ProjectionDetail, ProjectionPage } from '../../../../protocol/app-server/v48';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { ProjectionSummary } from './ContextSeat';

/** On-demand historical context inspection. Every row belongs to an activation,
 * never a Tool execution or an Agent Turn inferred from its visual position. */
export function ProjectionHistory({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const [page, setPage] = useState<ProjectionPage>();
  const [detail, setDetail] = useState<ProjectionDetail>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(false);
  async function load(after = '0') {
    setLoading(true); setError(undefined);
    try { const next = await client.projections(sessionId, after); setPage(next); }
    catch (failure) { setError(String(failure)); }
    finally { setLoading(false); }
  }
  async function inspect(checkpoint: string, offset = 0) {
    setLoading(true); setError(undefined);
    try { const next = await client.projectionDetail(sessionId, checkpoint, offset); setDetail(next); }
    catch (failure) { setError(String(failure)); }
    finally { setLoading(false); }
  }
  return <details data-projection-history="" onToggle={event => { if (event.currentTarget.open && !page && !loading) void load(); }}>
    <summary>{tx('agent:context.projection-history')}</summary>
    {error && <p role="alert">{error}</p>}
    {page?.records.map(head => <section key={head.activation_id}>
      <ProjectionSummary head={head}/>
      <button disabled={loading} onClick={() => { void inspect(head.checkpoint_id); }}>{tx('agent:context.projection-inspect')}</button>
    </section>)}
    {page?.next_activation && <button disabled={loading} onClick={() => { void load(page.next_activation!); }}>{tx('agent:context.projection-more')}</button>}
    {detail && <section data-projection-detail={detail.checkpoint.checkpoint_id}>
      {detail.items.map(item => <article key={item.occurrence.item_index}>
        <p>{item.occurrence.item_index + 1} · {item.content.role} · {item.content.provenance.type}</p>
        {item.content.blocks.map((block, index) => block.type === 'text'
          ? <MarkdownText key={index} text={block.text.text}/>
          : <pre key={index}>{JSON.stringify(block, null, 2)}</pre>)}
        {item.content.truncated && <p>{tx('agent:context.projection-truncated')}</p>}
      </article>)}
      {detail.next_item != null && <button disabled={loading} onClick={() => { void inspect(detail.checkpoint.checkpoint_id, detail.next_item!); }}>{tx('agent:context.projection-more')}</button>}
    </section>}
  </details>;
}
