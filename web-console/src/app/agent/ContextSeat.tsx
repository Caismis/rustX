import type { RuntimeClientContextView, RuntimeClientCompactionOperation } from '../../../../protocol/app-server/v82';
import type { AppServerClient } from '../../client/app-server';
import { compactionNoticeKey, inactiveCompaction } from '../../client/compaction-notices';
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
  if (request?.status === 'skipped' && !newerCommit) return { state: 'skipped' as const };
  if (request?.status === 'cancelled' && !newerCommit) return { state: 'cancelled' as const };
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
      dismissed: view?.snapshot && view.compactionNotices?.get(view.snapshot.conversation_id)?.dismissed,
      current: state.connection === 'connected' && view?.attachment === 'attached' };
  }, sameValue);
  const context = facts.current ? facts.context : undefined;
  const operations = context?.compaction_operations ?? [];
  const visible = operations.filter(operation => operation.status.phase !== 'committed' && !facts.dismissed?.has(compactionNoticeKey(operation)));
  const progress = context?.compaction_progress;
  const foreground = !!context?.compaction_in_progress && !operations.some(operation => operation.status.operation_id === progress?.operation_id);
  const joinedRequest = foreground && facts.request && ['submitting', 'uncertain'].includes(facts.request.status)
    && context?.manual_compaction?.request_id === facts.request.requestId && !context.manual_compaction.released;
  const manualContext = context && { ...context, compaction_in_progress: false };
  const pendingRequest = facts.request?.status === 'submitting' || facts.request?.status === 'uncertain';
  const presentation = foreground && (joinedRequest || !pendingRequest) ? undefined : compactionPresentation(manualContext, facts.request);
  const head = context?.projection_head;
  const manual = presentation?.state === 'succeeded' && head ? undefined : presentation;
  if (!visible.length && !foreground && !manual) return null;
  const foregroundCopy = progress?.mode === 'overflow' ? 'agent:context.overflow'
    : progress?.mode === 'blocking' ? 'agent:context.blocking'
      : progress?.mode === 'proactive' ? 'agent:context.generating' : 'agent:context.running';
  return <div data-context-seat="" role="status" aria-live="polite">
    {visible.map(operation => <OperationSeat key={compactionNoticeKey(operation)} operation={operation}/>) }
    {foreground && <div data-compaction-foreground="" data-compaction-operation={progress?.operation_id}>
      <CompactionMessage title={tx('agent:context.command-title')} summary={tx(foregroundCopy)} running>
        {joinedRequest && facts.request?.status === 'uncertain' && <p>{tx('agent:context.uncertain')}</p>}
      </CompactionMessage>
    </div>}
    {manual && <ManualSeat presentation={manual} client={client} sessionId={sessionId} current={facts.current}/>}
  </div>;
}

const operationLabels = {
  generating: 'agent:context.generating', retrying: 'agent:context.retrying',
  ready: 'agent:context.ready', foreground_waiting: 'agent:context.waiting',
  committed: 'agent:context.succeeded', failed: 'agent:context.summary-failed',
  cancelled: 'agent:context.cancelled', superseded: 'agent:context.superseded',
  no_effect: 'agent:context.no-effect',
} as const;

function OperationSeat({ operation }: { operation: RuntimeClientCompactionOperation }) {
  const tx = useTranslation(), { status, live } = operation;
  const running = live && ['generating', 'retrying', 'foreground_waiting'].includes(status.phase);
  return <div data-compaction-operation={status.operation_id} data-compaction-phase={status.phase} data-compaction-live={live}>
    <CompactionMessage title={tx('agent:context.summary-title')}
      summary={tx(inactiveCompaction(operation) ? 'agent:context.inactive' : operationLabels[status.phase])} running={running} error={status.phase === 'failed'}>
      {status.diagnostic && <p>{status.diagnostic}</p>}
    </CompactionMessage>
  </div>;
}

function ManualSeat({ presentation, client, sessionId, current }: {
  presentation: NonNullable<ReturnType<typeof compactionPresentation>>;
  client: AppServerClient; sessionId: string; current: boolean;
}) {
  const tx = useTranslation();
  // Committed checkpoints have their own historical transcript seats. Only
  // live operation/receipt feedback belongs at the current reading frontier.
  if (presentation.state === 'succeeded') return <CompactionMessage title={tx('agent:context.command-title')} summary={tx('agent:context.succeeded')}/>;
  if (presentation.state === 'skipped') return <div>{tx('agent:context.skipped')}</div>;
  if (presentation.state === 'cancelled') return <div>{tx('agent:context.cancelled')}</div>;
  const running = presentation.state === 'running' || presentation.state === 'submitting';
  const uncertain = presentation.state === 'uncertain';
  const diagnostic = presentation.diagnostic;
  return <div data-compaction-command="">
    <CompactionMessage title={tx('agent:context.command-title')} summary={[tx(running ? 'agent:context.running' : presentation.state === 'failed' ? 'agent:context.failed' : 'agent:context.uncertain'), running && uncertain ? tx('agent:context.uncertain') : undefined].filter(Boolean).join(' · ')} running={running} error={presentation.state === 'failed'}>
      {(diagnostic || uncertain) && <>
        {diagnostic && <p>{diagnostic}</p>}
        {uncertain && <p>{tx('agent:context.uncertain')} · {tx('agent:context.repair')}</p>}
        {current && <button className={css.read} type="button" onClick={() => { void client.refresh(sessionId).catch(() => {}); }}>{tx('agent:context.refresh')}</button>}
      </>}
    </CompactionMessage>
  </div>;
}
