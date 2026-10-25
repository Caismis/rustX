import { useEffect, useState } from 'react';
import type { RuntimeClientProjectionHeadView, TraceText } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { CompactionMessage } from '../../presentation/agent/Message';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';

/** A native checkpoint supplies summary content, never a second transcript. */
export function CompactionCheckpoint({ head, client, sessionId, title }: { head: RuntimeClientProjectionHeadView; client: AppServerClient; sessionId: string; title?: string }) {
  const tx = useTranslation(), [loaded, setLoaded] = useState<TraceText>();
  return <div data-compaction-checkpoint={head.checkpoint_id}>
    <CompactionMessage title={title ?? tx('agent:context.succeeded')} summary={tx(head.summary_preview ? 'agent:context.expand' : 'agent:context.summary-unavailable')}>
      {head.summary_preview && (loaded ? <><MarkdownText text={loaded.text}/>{loaded.truncated && <p>{tx('agent:context.summary-truncated')}</p>}</> : <SummaryContent key={`${sessionId}:${head.checkpoint_id}:${head.activation_id}`} head={head} client={client} sessionId={sessionId} onLoaded={setLoaded}/>) }
    </CompactionMessage>
  </div>;
}

/** Expanding a bounded preview reads only that immutable checkpoint's summary. */
function SummaryContent({ head, client, sessionId, onLoaded }: { head: RuntimeClientProjectionHeadView; client: AppServerClient; sessionId: string; onLoaded: (summary: TraceText) => void }) {
  const tx = useTranslation(), preview = head.summary_preview!;
  const [retry, setRetry] = useState(0);
  const [state, setState] = useState<{ text: string; truncated: boolean; error?: string }>();
  useEffect(() => {
    if (!preview.truncated) return;
    let mounted = true;
    const proof = client.getSnapshot().views[sessionId]?.attachmentObservation;
    const current = () => mounted && client.isAttachmentObservationCurrent(sessionId, proof);
    setState(undefined);
    if (!current()) { setState({ ...preview, error: tx('agent:context.summary-unavailable') }); return; }
    void client.projectionDetail(sessionId, head.checkpoint_id).then(detail => {
      if (!current()) return;
      if (detail.checkpoint.checkpoint_id !== head.checkpoint_id || detail.checkpoint.activation_id !== head.activation_id) throw Error(tx('agent:context.summary-unavailable'));
      const summary = detail.checkpoint.summary_preview;
      if (!summary) throw Error(tx('agent:context.summary-unavailable'));
      onLoaded(summary);
    }).catch(error => {
      if (current()) setState({ ...preview, error: String(error) });
    });
    return () => { mounted = false; };
  }, [head.checkpoint_id, head.activation_id, client, sessionId, preview.truncated, retry]);
  const summary: { text: string; truncated: boolean; error?: string } | undefined = preview.truncated ? state : preview;
  return <>
    <MarkdownText text={summary?.text ?? preview.text}/>
    {preview.truncated && !summary && <p role="status">{tx('trajectory:details.loading')}</p>}
    {summary?.error ? <p role="alert">{summary.error} <button type="button" onClick={() => setRetry(value => value + 1)}>{tx('agent:context.retry-summary')}</button></p>
      : summary?.truncated && <p>{tx('agent:context.summary-truncated')}</p>}
  </>;
}
