import type { Translate } from '../locale/translation';
import type { ClientView, SessionView } from '../client/app-server';

export type SessionRecovery = 'connect' | 'connection-settings' | 'open' | 'refresh';
export interface SessionProductState {
  status: 'idle' | 'working' | 'queued' | 'stopping' | 'connecting' | 'reconnect' | 'uncertain' | 'failure' | 'waiting';
  label?: string;
  detail?: string;
  severity: 'quiet' | 'warning' | 'error';
  recovery?: { action: SessionRecovery; label: string };
}

/** Pure display projection. Stale snapshots never imply current execution or
 * settlement. Recovery only names an existing client operation; no replay. */
export function deriveSessionProductState(tx: Translate, state: Pick<ClientView, 'connection' | 'uncertain'>, view?: SessionView, sessionId = view?.id): SessionProductState {
  if (view?.deleting) {
    if (view.error || state.uncertain.some(item => item.sessionId === sessionId)) return {
      status: 'uncertain', label: tx('common:session-product.deletion-needs-verification'), severity: 'warning',
      detail: view.error ?? tx('common:copy.the-delete-response-was-lost-reconnect-to-read-its-outcome-deletion-will-not-be-retried'),
      recovery: ['connecting', 'reconnecting', 'resynchronizing'].includes(state.connection) ? undefined : { action: 'connect', label: tx('common:session-product.reconnect-to-verify') },
    };
    return { status: 'stopping', label: tx('common:session-product.deleting-session'), severity: 'quiet' };
  }
  const connecting = ['connecting', 'reconnecting', 'resynchronizing'].includes(state.connection);
  const recovery: SessionProductState['recovery'] = connecting ? undefined
    : state.connection === 'incompatible' ? { action: 'connection-settings', label: tx('common:session-product.connection-settings') }
    : state.connection !== 'connected' ? { action: 'connect', label: tx('common:app.reconnect') }
    : !view || view.attachment === 'attaching' || view.attachment === 'resynchronizing' ? undefined
    : view.attachmentIntent !== 'wanted' || !view.target ? { action: 'open', label: tx('common:session-product.open-session') }
    : view.attachment !== 'attached' ? { action: 'refresh', label: tx('common:session-product.retry-connection') } : undefined;
  const uncertain = state.uncertain.some(item => sessionId !== undefined && item.sessionId === sessionId)
    || view?.cancellation?.status === 'uncertain' || view?.modelMutation?.status === 'uncertain'
    || view?.snapshot?.background?.some(tool => tool.state === 'outcome_unknown')
    || view?.snapshot?.attempt?.foreground?.some(tool => tool.state.type === 'settled' && tool.state.result.status.type === 'outcome_unknown')
    || view?.snapshot?.workflows?.runs.some(run => run.state.type === 'settled' && run.state.outcome === 'outcome_unknown');
  if (uncertain) return { status: 'uncertain', label: tx('common:session-product.needs-verification'), severity: 'warning', recovery,
    detail: tx('common:copy.an-operation-may-have-taken-effect-check-the-conversation-and-affected-work-before-trying-', { p0: view?.snapshot?.durability_failure ? tx('common:copy.storage-also-reported-a-failure-changes-may-not-be-saved') : '' }) };
  if (view?.snapshot?.durability_failure) return { status: 'failure', label: tx('common:session-product.changes-may-not-be-saved'), severity: 'error', recovery,
    detail: tx('common:copy.storage-reported-a-failure-resolve-the-storage-problem-before-continuing-inspect-the-recor') };
  if (state.connection === 'incompatible') return { status: 'failure', label: tx('common:session-product.connection-version-mismatch'), severity: 'error', recovery,
    detail: tx('common:copy.use-matching-web-and-server-versions-review-connection-settings-before-connecting-again') };
  if (recovery) return { status: 'reconnect', label: tx('common:session-product.connection-interrupted'), severity: 'warning', recovery,
    detail: recovery.action === 'open' ? tx('common:copy.open-this-session-to-continue-your-previous-requests-will-not-be-sent-again') : tx('common:copy.reconnect-to-see-current-work-your-previous-requests-will-not-be-sent-again') };
  if (connecting || view?.attachment === 'attaching' || view?.attachment === 'resynchronizing') return { status: 'connecting', label: tx('common:app.connecting'), severity: 'quiet' };
  if (!view?.snapshot) return { status: 'idle', severity: 'quiet' };
  const snapshot = view.snapshot;
  if (snapshot.shutting_down) return { status: 'stopping', label: tx('common:session-product.session-is-closing'), severity: 'warning' };
  if (view.cancellation) return { status: 'stopping', label: tx('common:session-product.stopping'), severity: 'quiet' };
  if (view.modelMutation) return { status: 'waiting', severity: 'quiet' };
  if (snapshot.pending_interactions?.length) return { status: 'waiting', label: tx('common:session-product.waiting-for-your-response'), severity: 'quiet' };
  if (snapshot.attempt && snapshot.attempt.phase.type !== 'settled') return { status: 'working', severity: 'quiet' };
  if (snapshot.inbound.pending?.length || view.submissions?.length) return { status: 'queued', label: tx('common:session-product.queued'), severity: 'quiet' };
  return { status: 'idle', severity: 'quiet' };
}
