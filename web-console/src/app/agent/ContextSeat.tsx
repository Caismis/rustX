import type { RuntimeClientContextView } from '../../../../protocol/app-server/v34';
import type { AppServerClient, CompactionRequestEvidence } from '../../client/app-server';
import { useClientSelector, sameValue } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { useState } from 'react';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { Modal } from '../../presentation/primitives/Modal';
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
  return <div className={css.root} data-context-seat="">
    {presentation && <div role="status" aria-live="polite">
      <span>{tx(`agent:context.${presentation.state}`)}</span>
      {presentation.state === 'running' && facts.request?.status === 'uncertain' && facts.context?.manual_compaction?.request_id !== facts.request.requestId && <p>{tx('agent:context.uncertain')} · {tx('agent:context.repair')}</p>}
      {presentation.state === 'running' && facts.request?.status === 'failed' && <details><summary>{tx('agent:context.failed')}</summary><p>{facts.request.diagnostic}</p></details>}
      {presentation.diagnostic && <details><summary>{tx('agent:context.details')}</summary><p>{presentation.diagnostic}</p></details>}
      {presentation.state === 'uncertain' && <p>{tx('agent:context.repair')}</p>}
      {facts.current && (presentation.state === 'uncertain' || presentation.state === 'failed') && <button className={css.read} type="button" onClick={() => { void client.refresh(sessionId).catch(() => {}); }}>{tx('agent:context.refresh')}</button>}
    </div>}
</div>;
}

export function ContextUsage({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const facts = useClientSelector(client, state => ({
    context: state.views[sessionId]?.snapshot?.context,
    current: state.connection === 'connected' && state.views[sessionId]?.attachment === 'attached',
  }), sameValue);
  const occupancy = facts.current ? facts.context?.last_request_occupancy : undefined;
  const known = occupancy && Number.isSafeInteger(occupancy.input_tokens) && occupancy.input_tokens >= 0
    && Number.isSafeInteger(occupancy.context_window_tokens) && occupancy.context_window_tokens > 0 && occupancy.model;
  const percent = known ? 100 * occupancy.input_tokens / occupancy.context_window_tokens : undefined;
  const displayPercent = percent === undefined ? undefined : percent > 0 && percent < 0.01 ? '<0.01' : new Intl.NumberFormat(tx.language, { maximumFractionDigits: 2 }).format(percent);
  const label = percent === undefined ? tx('agent:context.unavailable') : tx('agent:context.measured', { percent: displayPercent! });
  const detail = known ? tx('agent:context.tokens', { input: occupancy.input_tokens, capacity: occupancy.context_window_tokens, model: occupancy.model }) : tx('agent:context.explanation');
  return <>
    <Tooltip label={`${label} · ${detail}`} side="top"><button type="button" className={css.ringButton} aria-label={label} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}>
      <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7" fill="none" stroke="currentColor" strokeWidth="2" opacity="0.2" />
        {percent !== undefined && <circle cx="10" cy="10" r="7" fill="none" stroke="currentColor" strokeWidth="2" pathLength="100" strokeDasharray={`${Math.min(100, percent)} 100`} transform="rotate(-90 10 10)" />}
        {percent === undefined && <text x="10" y="10" textAnchor="middle" dominantBaseline="central" fill="currentColor" fontFamily="var(--dsw-font-family)" fontSize="9">?</text>}
      </svg>
    </button></Tooltip>
    <Modal open={open} title={label} closeLabel={tx('agent:turn-tail.close-usage')} onClose={() => setOpen(false)}>
      {known && <p>{detail}</p>}
      <p>{tx('agent:context.explanation')}</p>
    </Modal>
  </>;
}
