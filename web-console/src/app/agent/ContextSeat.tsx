import type { RuntimeClientContextView } from '../../../../protocol/app-server/v35';
import type { AppServerClient, CompactionRequestEvidence } from '../../client/app-server';
import { useClientSelector, sameValue } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import css from './ContextSeat.module.css';

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
  const occupancy = facts.current ? facts.context?.last_request_occupancy : undefined;
  const known = occupancy && Number.isSafeInteger(occupancy.input_tokens) && occupancy.input_tokens >= 0
    && Number.isSafeInteger(occupancy.context_window_tokens) && occupancy.context_window_tokens > 0 && occupancy.model;
  const percent = known ? Math.round(100 * occupancy.input_tokens / occupancy.context_window_tokens) : undefined;
  return <div className={css.root} data-context-seat="">
    {presentation && <div role="status" aria-live="polite">
      <span>{tx(`agent:context.${presentation.state}`)}</span>
      {presentation.state === 'running' && facts.request?.status === 'uncertain' && facts.context?.manual_compaction?.request_id !== facts.request.requestId && <p>{tx('agent:context.uncertain')} · {tx('agent:context.repair')}</p>}
      {presentation.state === 'running' && facts.request?.status === 'failed' && <details><summary>{tx('agent:context.failed')}</summary><p>{facts.request.diagnostic}</p></details>}
      {presentation.diagnostic && <details><summary>{tx('agent:context.details')}</summary><p>{presentation.diagnostic}</p></details>}
      {presentation.state === 'uncertain' && <p>{tx('agent:context.repair')}</p>}
      {facts.current && (presentation.state === 'uncertain' || presentation.state === 'failed') && <button className={css.read} type="button" onClick={() => { void client.refresh(sessionId).catch(() => {}); }}>{tx('agent:context.refresh')}</button>}
    </div>}
    <details className={css.measurement}>
      <summary>{known && <meter className={css.meter} min={0} max={occupancy.context_window_tokens} value={occupancy.input_tokens} aria-label={tx('agent:context.measured', { percent: percent! })}/>} {percent === undefined ? tx('agent:context.unavailable') : tx('agent:context.measured', { percent })}</summary>
      {known && <p>{tx('agent:context.tokens', { input: occupancy.input_tokens, capacity: occupancy.context_window_tokens, model: occupancy.model })}</p>}
      <p>{tx('agent:context.explanation')}</p>
    </details>
  </div>;
}
