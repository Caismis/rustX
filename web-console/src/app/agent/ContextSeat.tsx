import type { RuntimeClientContextView } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import type { CompactionRequestEvidence } from '../../client/contracts';
import { useClientSelector, sameValue } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { CompactionMessage } from '../../presentation/agent/Message';
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

/** Compaction lives in the Chat reading flow, alongside the messages it follows. */
export function ContextSeat({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const facts = useClientSelector(client, state => {
    const view = state.views[sessionId];
    return { context: view?.snapshot?.context, request: view?.compactionRequest,
      current: state.connection === 'connected' && view?.attachment === 'attached' };
  }, sameValue);
  const presentation = compactionPresentation(facts.current ? facts.context : undefined, facts.request);
  const head = facts.current ? facts.context?.projection_head : undefined;
  // Committed checkpoints have their own historical transcript seats. Only
  // live operation/receipt feedback belongs at the current reading frontier.
  if (!presentation || presentation.state === 'succeeded') return presentation && !head ? <div role="status"><CompactionMessage title={tx('agent:context.command-title')} summary={tx('agent:context.succeeded')}/></div> : null;
  const running = presentation.state === 'running' || presentation.state === 'submitting';
  const uncertain = presentation.state === 'uncertain' || presentation.state === 'running' && facts.request?.status === 'uncertain' && facts.context?.manual_compaction?.request_id !== facts.request.requestId;
  const diagnostic = presentation.diagnostic ?? (presentation.state === 'running' && facts.request?.status === 'failed' ? facts.request.diagnostic : undefined);
  return <div data-context-seat="" role="status" aria-live="polite">
    <CompactionMessage title={tx('agent:context.command-title')} summary={[tx(running ? 'agent:context.running' : presentation.state === 'failed' ? 'agent:context.failed' : 'agent:context.uncertain'), running && uncertain ? tx('agent:context.uncertain') : undefined].filter(Boolean).join(' · ')} running={running} error={presentation.state === 'failed'}>
      {(diagnostic || uncertain) && <>
        {diagnostic && <p>{diagnostic}</p>}
        {uncertain && <p>{tx('agent:context.uncertain')} · {tx('agent:context.repair')}</p>}
        {facts.current && <button className={css.read} type="button" onClick={() => { void client.refresh(sessionId).catch(() => {}); }}>{tx('agent:context.refresh')}</button>}
      </>}
    </CompactionMessage>
  </div>;
}
