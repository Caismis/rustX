import { ProjectionHistory } from "./ProjectionHistory";
import type { RuntimeClientContextView } from '../../../../protocol/app-server/v52';
import type { AppServerClient, CompactionRequestEvidence } from '../../client/app-server';
import { useClientSelector, sameValue } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import css from './ContextSeat.module.css';
import { CompactionMessage } from '../../presentation/agent/Message';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { markdownPreviewText } from '../../presentation/markdown/preview';

/** An independent projection disclosure; it is never placed in a canonical turn. */
export function ProjectionSummary({ head }: { head: NonNullable<RuntimeClientContextView['projection_head']> }) {
  const tx = useTranslation();
  const inherited = head.destination_installation != null;
  const text = head.summary_preview?.text ?? '';
  return <div data-projection-checkpoint={head.checkpoint_id} data-projection-origin={inherited ? 'inherited' : 'local'}>
    <CompactionMessage title={tx(inherited ? 'agent:context.inherited-projection' : 'agent:context.local-projection')}
      summary={tx('agent:context.projection-boundary', { activation: head.activation_id, ledger: head.base_frontier })}
      preview={markdownPreviewText(text)} description={tx('agent:context.projection-description')}>
      <p>{tx('agent:context.projection-origin', { conversation: head.origin.conversation_id, activation: head.origin.activation })}</p>
      <MarkdownText text={text}/>
      {head.summary_preview?.truncated && <p>{tx('agent:context.projection-truncated')}</p>}
    </CompactionMessage>
  </div>;
}


/** Request evidence never substitutes for an authoritative native lifecycle. */
export function compactionPresentation(context?: RuntimeClientContextView, request?: CompactionRequestEvidence) {
  if (context?.compaction_in_progress) return { state: 'running' as const };
  if (request?.status === 'submitting') return { state: 'submitting' as const };
  if (request?.status === 'uncertain') return { state: 'uncertain' as const };
  const newerCommit = !!request && (context?.compaction_count ?? 0) > request.baselineCount;
  if (request?.status === 'succeeded' && !newerCommit) return { state: 'succeeded' as const };
  if (request?.status === 'failed' && !newerCommit) return { state: 'failed' as const, diagnostic: request.diagnostic };
  if (context?.compaction_error) return { state: 'failed' as const, diagnostic: context.compaction_error };
  if (request?.status === 'succeeded' || context?.latest_compaction) return { state: 'succeeded' as const };
  return undefined;
}

export function ContextSeat({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const facts = useClientSelector(client, state => {
    const view = state.views[sessionId];
    return { context: view?.snapshot?.context, request: view?.compactionRequest,
      current: state.connection === 'connected' && view?.attachment === 'attached' };
  }, sameValue);
  const presentation = compactionPresentation(facts.current ? facts.context : undefined, facts.request);
  const head = facts.current ? facts.context?.projection_head : undefined;
  if (!presentation || presentation.state === 'succeeded') {
    return head ? <div><ProjectionSummary key={head.checkpoint_id} head={head}/><ProjectionHistory key={sessionId} client={client} sessionId={sessionId}/></div> : presentation ? <div role="status">{tx('agent:context.succeeded')}</div> : null;
  }
  return <div className={css.root} data-context-seat="">
    <div role="status" aria-live="polite">
      <span>{tx(`agent:context.${presentation.state}`)}</span>
      {presentation.state === 'running' && facts.request?.status === 'uncertain' && facts.context?.manual_compaction?.request_id !== facts.request.requestId && <p>{tx('agent:context.uncertain')} · {tx('agent:context.repair')}</p>}
      {presentation.state === 'running' && facts.request?.status === 'failed' && <details><summary>{tx('agent:context.failed')}</summary><p>{facts.request.diagnostic}</p></details>}
      {presentation.diagnostic && <details><summary>{tx('agent:context.details')}</summary><p>{presentation.diagnostic}</p></details>}
      {presentation.state === 'uncertain' && <p>{tx('agent:context.repair')}</p>}
      {facts.current && (presentation.state === 'uncertain' || presentation.state === 'failed') && <button className={css.read} type="button" onClick={() => { void client.refresh(sessionId).catch(() => {}); }}>{tx('agent:context.refresh')}</button>}
    </div>
    {head && <ProjectionSummary key={head.checkpoint_id} head={head}/>}
  </div>;
}
